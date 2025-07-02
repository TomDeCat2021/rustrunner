// Copyright 2019 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#ifndef _GNU_SOURCE
#define _GNU_SOURCE 1
#endif

#ifdef __APPLE__
#include <sys/mman.h>
#endif

#ifdef __linux__
#include <linux/memfd.h>
#include <sys/syscall.h>
#ifndef MFD_CLOEXEC
#define MFD_CLOEXEC 0x0001U
#endif
#endif

#include "reprl.h"
#include <fcntl.h>

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

// Well-known file descriptor numbers for reprl <-> child communication, child process side
// Fuzzilli REPRL fd numbers
#define REPRL_CHILD_CTRL_IN 100   // REPRL_CRFD in Fuzzilli
#define REPRL_CHILD_CTRL_OUT 101  // REPRL_CWFD in Fuzzilli
#define REPRL_CHILD_DATA_IN 102   // REPRL_DRFD in Fuzzilli
#define REPRL_CHILD_DATA_OUT 103  // REPRL_DWFD in Fuzzilli

// #define SHM_SIZE 0x100000			// the size must be big enough for the target JS engine (v8)
// #define MAX_EDGES ((SHM_SIZE - 4) * 8)
#define unlikely(cond) __builtin_expect(!!(cond), 0)


#define MIN(x, y) ((x) < (y) ? (x) : (y))

/// Maximum timeout in microseconds. Mostly just limited by the fact that the timeout in milliseconds has to fit into a 32-bit integer.
#define REPRL_MAX_TIMEOUT_IN_MICROSECONDS ((uint64_t)(INT_MAX) * 1000)

struct cov_context contexts[512] = {0};

// char **prog_argv = NULL;
// char **environment = NULL;
int environment_length = 0;
extern char **environ;

struct reprl_context* reprl_contexts[128] = {NULL}; // Array to store contexts for each worker



static_assert(MAX_EDGES <= UINT32_MAX, "Edges must be addressable using a 32-bit index");

static inline int edge(const uint8_t* bits, uint64_t index)
{
    return (bits[index / 8] >> (index % 8)) & 0x1;
}

static inline void set_edge(uint8_t* bits, uint64_t index)
{
    // printf("Before setting edge at index %ld %d Edge %d\n", index, bits[index / 8], edge(bits, index));
    bits[index / 8] |= 1 << (index % 8);
    // printf("After setting edge at index %ld %d Edge %d\n", index, bits[index / 8], edge(bits, index));
}

static inline void clear_edge(uint8_t* bits, uint64_t index)
{
    // printf("Before clearing edge at index %ld %d Edge %d\n", index, bits[index / 8], edge(bits, index));
    bits[index / 8] &= ~(1u << (index % 8));
    // printf("After clearing edge at index %ld %d Edge %d\n", index, bits[index / 8], edge(bits, index));
}
static uint64_t current_usecs()
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000000 + ts.tv_nsec / 1000;
}

static char** copy_string_array(const char** orig)
{
    size_t num_entries = 0;
    for (const char** current = orig; *current; current++) {
        num_entries += 1;
    }
    char** copy = calloc(num_entries + 1, sizeof(char*));
    for (size_t i = 0; i < num_entries; i++) {
        copy[i] = strdup(orig[i]);
    }
    return copy;
}

static void free_string_array(char** arr)
{
    if (!arr) return;
    for (char** current = arr; *current; current++) {
        free(*current);
    }
    free(arr);
}


static int reprl_error(struct reprl_context* ctx, const char *format, ...)
{
    (void)ctx; // Suppress unused parameter warning
    va_list args;
    va_start(args, format);
    // free(ctx->last_error);
    // vasprintf(&ctx->last_error, format, args);
    // fprintf(stderr, "REPRL error: %s\n", ctx->last_error);
    return -1;
}

static struct data_channel* reprl_create_data_channel(struct reprl_context* ctx, int worker_id)
{
    char channel_name[64];
    snprintf(channel_name, sizeof(channel_name), "REPRL_DATA_CHANNEL_%d", worker_id);

    int fd;
#ifdef __linux__
    fd = memfd_create(channel_name, MFD_CLOEXEC);
#else
    // On macOS, create a temporary file that can be read/written normally
    char temp_name[256];
    snprintf(temp_name, sizeof(temp_name), "/tmp/reprl_%d_%d_XXXXXX", getpid(), worker_id);
    
    // mkstemp creates and opens a unique temporary file
    fd = mkstemp(temp_name);
    if (fd != -1) {
        // Set close-on-exec flag
        fcntl(fd, F_SETFD, FD_CLOEXEC);
        // Remove the file immediately so it becomes anonymous
        unlink(temp_name);
    }
#endif
    if (fd == -1) {
        reprl_error(ctx, "Failed to create data channel file: %s", strerror(errno));
        return NULL;
    }
    
    // printf("Created data channel fd=%d for worker %d, attempting ftruncate to size %d\n", fd, worker_id, REPRL_MAX_DATA_SIZE);
    
    if (ftruncate(fd, REPRL_MAX_DATA_SIZE) != 0) {
        fprintf(stderr, "Failed to ftruncate data channel (fd=%d, size=%d): %s (errno=%d)\n", fd, REPRL_MAX_DATA_SIZE, strerror(errno), errno);
        close(fd);
        return NULL;
    }
    char* mapping = mmap(0, REPRL_MAX_DATA_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (mapping == MAP_FAILED) {
        reprl_error(ctx, "Failed to mmap data channel file: %s", strerror(errno));
        return NULL;
    }

    struct data_channel* channel = malloc(sizeof(struct data_channel));
    channel->fd = fd;
    channel->mapping = mapping;
    return channel;
}

static void reprl_destroy_data_channel(struct reprl_context* ctx, struct data_channel* channel)
{
    (void)ctx; // Suppress unused parameter warning
    if (!channel) return;
    close(channel->fd);
    munmap(channel->mapping, REPRL_MAX_DATA_SIZE);
    free(channel);
}

static void reprl_child_terminated(struct reprl_context* ctx)
{
    if (!ctx->pid) return;
    ctx->pid = 0;
    close(ctx->ctrl_in);
    close(ctx->ctrl_out);
}

static void reprl_terminate_child(struct reprl_context* ctx)
{
    if (!ctx->pid) return;
    int status;
    kill(ctx->pid, SIGKILL);
    waitpid(ctx->pid, &status, 0);
    reprl_child_terminated(ctx);
}

static inline int coverage_is_edge_set(const uint8_t* bits, uint64_t index) {
    return (bits[index / 8] >> (index % 8)) & 0x1;
}

// A zero means edge is set in virgin map
static inline int virgin_is_edge_set(const uint8_t* bits, uint64_t index) {
    return 1 - coverage_is_edge_set(bits, index);
}


// In Virgin map an edge is a 0 and not a 1
static int get_number_edges_virgin(uint64_t* start, uint64_t* end) {
	uint64_t* current = start;
	int tmp_count_edges = 0;
	while (current < end) {
		uint64_t index = ((uintptr_t)current - (uintptr_t)start) * 8;
		for (uint64_t i = index; i < index + 64; i++) {
			if (virgin_is_edge_set((const uint8_t*)start, i) == 1) {
				tmp_count_edges += 1;
			}
		}
		current++;
	}
	return tmp_count_edges;
}
int coverage_save_virgin_bits_in_file(int worker_id, const char *filepath) {
	FILE *write_ptr= fopen(filepath,"wb");  // w for write, b for binary
    if (write_ptr == NULL) {
        printf("Failed to open file %s\n", filepath);
        return -1;
    }
    struct cov_context* context = &contexts[worker_id];
    if (context->virgin_bits == NULL) {
        printf("Virgin bits are NULL for worker %d\n", worker_id);
        return -1;
    }
	fwrite(context->virgin_bits,context->bitmap_size,1,write_ptr);

	fclose(write_ptr);
    return get_number_edges_virgin((uint64_t*)context->virgin_bits, (uint64_t*)(context->virgin_bits + context->bitmap_size));
}
void coverage_backup_virgin_bits(int worker_id) {
    struct cov_context* context = &contexts[worker_id];
    if (context->virgin_bits == NULL) {
        printf("Virgin bits are NULL for worker %d\n", worker_id);
        return;
    }
	memcpy(context->virgin_bits_backup, context->virgin_bits, context->bitmap_size);

}
int coverage_load_virgin_bits_from_file(int worker_id,const char *filepath) {
	FILE *ptr = fopen(filepath,"rb");
    if (ptr == NULL) {
        printf("Failed to open file %s\n", filepath);
        return -1;
    }

    struct cov_context* context = &contexts[worker_id];
    if (context == NULL) {
        printf("Context is NULL for worker %d\n", worker_id);
        return -1;
    }
    if (context->virgin_bits == NULL) {
        printf("Virgin bits are NULL for worker %d\n", worker_id);
        return -1;
    }
	if (fread(context->virgin_bits, context->bitmap_size,1,ptr) == 0) {
	    // This error occurs when you update the JS engine and try to load an old coverage map with a new JS engine
		fprintf(stderr, "Fread() error in coverage_load_virgin_bits_from_file(). Was the coverage map created with this JS engine?\n");
 		exit(-1);
	}
	coverage_backup_virgin_bits(worker_id);
	fclose(ptr);

    coverage_clear_bitmap(worker_id);    // This call is important: Otherwise an execute-call after the load virgin bits call will lead to incorrect results 

	return get_number_edges_virgin((uint64_t*)context->virgin_bits, (uint64_t*)(context->virgin_bits + context->bitmap_size));
}



// Restores the virgin bits to the original value or to the value stored via the
// coverage_backup_virgin_bits() function call
void coverage_restore_virgin_bits(int worker_id) {
	memcpy(contexts[worker_id].virgin_bits, contexts[worker_id].virgin_bits_backup, contexts[worker_id].bitmap_size);
}

void coverage_shutdown(int worker_id) {
    struct cov_context* context = &contexts[worker_id];
    char shm_key[1024];
    snprintf(shm_key, 1024, "/shm_id_%d_%d", getpid(), context->id);
    shm_unlink(shm_key);
}

// ================ Start helper functions ==================
static char *dup_str(const char *str) {
	size_t len = strlen(str) + 1;
	char *dup = malloc(len);
	if (dup != NULL)
		memmove(dup, str, len);
	return dup;
}
// ================ End helper functions ==================





int coverage_initialize(int shm_id) { // worker_id
    printf("Initializing coverage for worker %d\n", shm_id);
    struct cov_context* context = &contexts[shm_id];
	char shm_key[1024];
	snprintf(shm_key, 1024, "/shm_id_%d_%d", getpid(), shm_id);
	context->id = shm_id;
	if(context->shmem != NULL) {
		coverage_shutdown(shm_id);
	}
	
	// First unlink any existing shared memory with this name
	shm_unlink(shm_key);
	
	int fd = shm_open(shm_key, O_RDWR | O_CREAT | O_EXCL, 0600);
	if (fd == -1) {
		fprintf(stderr, "Failed to create shared memory region '%s': %s\n", shm_key, strerror(errno));
		return -1;
	}
	
	// Debug info
	printf("Created shm fd %d for key %s, attempting ftruncate to size %d\n", fd, shm_key, SHM_SIZE);
	
	int tmp_ret = ftruncate(fd, SHM_SIZE);
	if(tmp_ret != 0) {
		fprintf(stderr, "ftruncate() failed for fd %d, size %d: %s (errno=%d)\n", 
		        fd, SHM_SIZE, strerror(errno), errno);
		close(fd);
		shm_unlink(shm_key);
		return -1;
	}

	if(context->shmem != NULL) {
		munmap(context->shmem, SHM_SIZE);
	}
	context->shmem = mmap(0, SHM_SIZE, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	close(fd);

	// The correct bitmap size is calculated in the >coverage_finish_initialization< function
	// This function must be called after the first execution, however, the first execution
	// Already uses the bitmap_size. I therefore set it here to zero so that the
	// coverage_clear_bitmap() function does not write something in the first call
	context->bitmap_size	= 0;
    context->virgin_bits = NULL;
	return 0;
}


uint32_t coverage_finish_initialization(int worker_id, int should_track_edges) {
    struct cov_context* context = &contexts[worker_id];
    uint32_t num_edges = context->shmem->num_edges;
    if (num_edges == 0) {
        fprintf(stderr, "[LibCoverage] Coverage bitmap size could not be determined, is the engine instrumentation working properly?\n");
        exit(-1);
    }
    // Llvm's sanitizer coverage ignores edges whose guard is zero, and our instrumentation stores the bitmap indices in the guard values.
    // To keep the coverage instrumentation as simple as possible, we simply start indexing edges at one and thus ignore the zeroth edge.
    num_edges += 1;
    if(context->virgin_bits != NULL) {
		free(context->virgin_bits);

	}


    if (num_edges > MAX_EDGES) {
        exit(-1);           // TODO
    }
    // Compute the bitmap size in bytes required for the given number of edges and
    // make sure that the allocation size is rounded up to the next 8-byte boundary.
    // We need this because evaluate iterates over the bitmap in 8-byte words.
    uint32_t bitmap_size = (num_edges + 7) / 8;
    bitmap_size += (7 - ((bitmap_size - 1) % 8));

    context->num_edges = num_edges;
    context->bitmap_size = bitmap_size;

    context->should_track_edges = should_track_edges;

    context->virgin_bits = malloc(bitmap_size);
    context->virgin_bits_backup = malloc(bitmap_size);
    context->coverage_map_backup = malloc(bitmap_size);

    // context.crash_bits = malloc(bitmap_size);
    memset(context->virgin_bits, 0xff, bitmap_size);
    // memset(context->crash_bits, 0xff, bitmap_size);
    context->edge_count = NULL;

    // Zeroth edge is ignored, see above.
    clear_edge(context->virgin_bits, 0);
    // clear_edge(context->crash_bits, 0);
}

static uint32_t internal_evaluate(struct cov_context* context, uint8_t* virgin_bits, struct edge_set* new_edges)
{
    uint64_t* current = (uint64_t*)context->shmem->edges;
    uint64_t* end = (uint64_t*)(context->shmem->edges + context->bitmap_size);
    uint64_t* virgin = (uint64_t*)virgin_bits;
    new_edges->count = 0;
    new_edges->edge_indices = NULL;

    // Perform the initial pass regardless of the setting for tracking how often invidual edges are hit
    while (current < end) {
        if (*current && unlikely(*current & *virgin)) {
            // New edge(s) found!
            // We know that we have <= UINT32_MAX edges, so every index can safely be truncated to 32 bits.
            uint64_t index = (uint64_t)((uintptr_t)current - (uintptr_t)context->shmem->edges) * 8;
            for (uint64_t i = index; i < index + 64; i++) {
                if (edge(context->shmem->edges, i) == 1 && edge(virgin_bits, i) == 1) {
                    clear_edge(virgin_bits, i);
                    new_edges->count += 1;
                    size_t new_num_entries = new_edges->count;
                    new_edges->edge_indices = realloc(new_edges->edge_indices, new_num_entries * sizeof(uint64_t));
                    new_edges->edge_indices[new_edges->count - 1] = i;
                }
            }
        }

        current++;
        virgin++;
    }

    return new_edges->count;
}


int cov_evaluate(int worker_id,struct edge_set* new_edges  )
{
    struct cov_context* context = &contexts[worker_id];
    uint32_t num_new_edges = internal_evaluate(context, context->virgin_bits ,new_edges);
    return num_new_edges ;
}

// struct CmpEvent* cov_fetch_cmp_events(int worker_id) {
//     struct cov_context* context = &contexts[worker_id];
//     return context->shmem->g_cmp_events;
// }

// uint64_t fetch_event_count(int worker_id) {
//     struct cov_context* context = &contexts[worker_id];
//     return context->shmem->event_count;
// }

// void cov_clear_cmp_events(int worker_id) {
//     struct cov_context* context = &contexts[worker_id];
//     context->shmem->event_count = 0;
//     memset(context->shmem->g_cmp_events, 0, sizeof(struct CmpEvent) * context->shmem->event_count);
// }
void init(int worker_id){
    printf("Worker %d Initializing\n", worker_id);
    char **prog_argv = malloc(20 * sizeof(char *));
    int arg_idx = 0;
    // char *target_path = getenv("TARGET"); // Unused variable
    char *target = getenv("TARGET");
    char *bin_path = getenv("BIN");
    char *compiler = getenv("BASELINE");

    if (target == NULL || bin_path == NULL) {
        fprintf(stderr, "ERROR: TARGET and BIN environment variables must be set\n");
        exit(1);
    }

    // Set binary path as first argument
    prog_argv[arg_idx++] = bin_path;

    // Configure arguments based on target engine
    if (strcmp(target, "v8") == 0) {
        // V8 specific arguments
        prog_argv[arg_idx++] = "--allow-natives-syntax";
        prog_argv[arg_idx++] = "--expose-gc";
        prog_argv[arg_idx++] = "--fuzzing";
        prog_argv[arg_idx++] = "--harmony-temporal";
        if ( worker_id > 100) {
            prog_argv[arg_idx++] = "--print-bytecode";
        }
    }
    else if (strcmp(target, "firefox") == 0) {
        // Firefox specific arguments
        prog_argv[arg_idx++] = "--baseline-warmup-threshold=10";
        prog_argv[arg_idx++] = "--ion-warmup-threshold=100";
        prog_argv[arg_idx++] = "--ion-check-range-analysis";
        prog_argv[arg_idx++] = "--ion-extra-checks";
        prog_argv[arg_idx++] = "--fuzzing-safe";
        prog_argv[arg_idx++] = "--disable-oom-functions";
        
        // Set compiler based on BASELINE env var
        if (compiler == NULL) {
            prog_argv[arg_idx++] = "--wasm-compiler=ion";
        } else {
            prog_argv[arg_idx++] = "--wasm-compiler=baseline";
        }
        prog_argv[arg_idx++] = "--reprl";
    }
    else if (strcmp(target, "jsc") == 0) {
        // JavaScriptCore specific arguments
        prog_argv[arg_idx++] = "--validateAsYouParse=true";
        prog_argv[arg_idx++] = "--useConcurrentJIT=false";
        prog_argv[arg_idx++] = "--thresholdForJITAfterWarmUp=10";
        prog_argv[arg_idx++] = "--thresholdForJITSoon=10";
        prog_argv[arg_idx++] = "--thresholdForOptimizeAfterWarmUp=100";
        prog_argv[arg_idx++] = "--thresholdForOptimizeAfterLongWarmUp=100";
        prog_argv[arg_idx++] = "--thresholdForOptimizeSoon=100";
        prog_argv[arg_idx++] = "--thresholdForFTLOptimizeAfterWarmUp=1000";
        prog_argv[arg_idx++] = "--future";
        prog_argv[arg_idx++] = "--enableWebAssembly=true";
        prog_argv[arg_idx++] = "--useWebAssemblyFastMemory=true";
        prog_argv[arg_idx++] = "--reprl";
    }
    else {
        fprintf(stderr, "ERROR: Unknown target engine: %s\n", target);
        exit(1);
    }

    // Null terminate argument list
    prog_argv[arg_idx] = NULL;

    // Debug print arguments
    printf("Running %s with arguments:\n", target);
    for (int i = 0; i < arg_idx; i++) {
        printf("  arg[%d]: %s\n", i, prog_argv[i]);
    }


    int shm_id = worker_id;
	// Now copy the environment
	// Count environment variables
    char **env = environ;
    int env_count = 0;
    while (env[env_count] != NULL) {
        env_count++;
    }

    // Allocate memory for the new environment array
    char **new_env = (char**)malloc((env_count + 1) * sizeof(char*));
    if (new_env == NULL) {
        perror("Failed to allocate memory for environment");
        exit(1);
    }

    // Copy each environment string
    for (int i = 0; i < env_count; i++) {
        new_env[i] = strdup(environ[i]);
        if (new_env[i] == NULL) {
            perror("Failed to copy environment variable");
            // Clean up already allocated strings
            for (int j = 0; j < i; j++) {
                free(new_env[j]);
            }
            free(new_env);
            exit(1);
        }
    }
    new_env[env_count] = NULL;  // Null terminate the array

	// Use new_env instead of env
	int listSZ;
	for (listSZ = 0; new_env[listSZ] != NULL; listSZ++) { }
	//printf("DEBUG: Number of environment variables = %d\n", listSZ);
	listSZ += 2;	// One more environment variable for the shared memory; One for null termination
    printf("Worker %d Allocating environment\n", worker_id);
    char **environment = malloc(listSZ * sizeof(char *));

	// if(environment_length != 0 && environment != NULL) {
	// 	// Free previous allocations
	// 	for (int i = 0; i < (listSZ-1); i++) {
	// 		free(environment[i]);
	// 	}
	// 	free(environment);
	// }

	environment_length = listSZ;
	environment = malloc(listSZ * sizeof(char *));
	if (environment == NULL) {
		fprintf(stderr, "[libJSEngine] Memory allocation failed!\n");
 		exit(-1);
	}
	for (int i = 0; i < (listSZ-2); i++) {
		if ((environment[i] = dup_str(new_env[i])) == NULL) {
			fprintf(stderr, "[libJSEngine] Memory allocation failed!\n");
			exit(-1);
		}
	}

	char shm_key[1024];
	snprintf(shm_key, 1024, "SHM_ID=/shm_id_%d_%d", getpid(), shm_id);
	environment[listSZ-2] = dup_str(shm_key);
	environment[listSZ-1] = NULL;
    printf("Worker %d Creating reprl context\n", worker_id);
    struct reprl_context* current_reprl_context = reprl_create_context();
    reprl_contexts[worker_id] = current_reprl_context;
    int ret = reprl_initialize_context(current_reprl_context, prog_argv, environment, 1, 1, worker_id);	// capture: stdout=true; stderr=true

    if(ret == -1) {
		fprintf(stderr, "[libJSEngine] reprl_initialize_context() failed!\n");
		exit(-1);
	}

    coverage_initialize( shm_id);		// Initialize the coverage map
    printf("Worker %d Initialized\n", worker_id);

}

static int reprl_spawn_child(struct reprl_context* ctx);


void spawn(int worker_id) {
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    int ret = reprl_spawn_child(current_reprl_context);
    fprintf(stderr, "[libJSEngine] Spawning child process...\n");
    if(ret == -1) {
        fprintf(stderr, "[libJSEngine] reprl_spawn_child() failed!\n");
        exit(-1);
    }
    else{
        printf("[libJSEngine] Child process spawned successfully!\n");
    }
}

int execute_script(char* arg_script_string, int arg_timeout, int fresh_instance, int worker_id){
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    uint64_t real_execution_time = 0;
    if (arg_script_string == NULL) {
        return -1;
    }
    int return_value = 0;
    int arg_script_length = strlen(arg_script_string);
    
    // printf("execute_script: worker_id=%d, script='%s', length=%d, timeout=%d\n", 
    //        worker_id, arg_script_string, arg_script_length, arg_timeout);

    arg_timeout = arg_timeout * 1000;
    return_value = reprl_execute(current_reprl_context, arg_script_string, (int64_t)(arg_script_length), (int64_t)(arg_timeout), &real_execution_time, fresh_instance, worker_id);


    // Fetch and print stdout
    // char* stdout_content = reprl_fetch_stdout(worker_id); // Unused variable
    // if (stdout_content && *stdout_content) {
    //     printf("Stdout:\n%s\n", stdout_content);
    // }

    // printf("Exec: workerid %d -> return value: %d\n", worker_id, return_value);

    return return_value;
}



static int reprl_spawn_child(struct reprl_context* ctx)
{
	int ret = 0;
#ifdef __linux__
    // This is also a good time to ensure the data channel backing files don't grow too large.
    // On Linux with memfd, we can always ftruncate
    ret = ftruncate(ctx->data_in->fd, REPRL_MAX_DATA_SIZE);
	if(ret != 0) { fprintf(stderr, "ftruncate(data_in->fd=%d, size=%d) failed: %s\n", ctx->data_in->fd, REPRL_MAX_DATA_SIZE, strerror(errno)); return -1;}
    ret = ftruncate(ctx->data_out->fd, REPRL_MAX_DATA_SIZE);
	if(ret != 0) { fprintf(stderr, "ftruncate(data_out->fd=%d, size=%d) failed: %s\n", ctx->data_out->fd, REPRL_MAX_DATA_SIZE, strerror(errno)); return -1;}
    if (ctx->stdout) {
		ret = ftruncate(ctx->stdout->fd, REPRL_MAX_DATA_SIZE);
		if(ret != 0) { fprintf(stderr, "ftruncate(stdout->fd=%d, size=%d) failed: %s\n", ctx->stdout->fd, REPRL_MAX_DATA_SIZE, strerror(errno)); return -1;}
	}
    if (ctx->stderr) {
		ret = ftruncate(ctx->stderr->fd, REPRL_MAX_DATA_SIZE);
		if(ret != 0) { fprintf(stderr, "ftruncate(stderr->fd=%d, size=%d) failed: %s\n", ctx->stderr->fd, REPRL_MAX_DATA_SIZE, strerror(errno)); return -1;}
	}
#else
    // On macOS, after shm_unlink the fds become anonymous and ftruncate may fail
    // We already sized them correctly in reprl_create_data_channel, so just reset the file position
    lseek(ctx->data_in->fd, 0, SEEK_SET);
    lseek(ctx->data_out->fd, 0, SEEK_SET);
    if (ctx->stdout) {
        lseek(ctx->stdout->fd, 0, SEEK_SET);
    }
    if (ctx->stderr) {
        lseek(ctx->stderr->fd, 0, SEEK_SET);
    }
#endif

    int crpipe[2] = { 0, 0 };          // control pipe child -> reprl
    int cwpipe[2] = { 0, 0 };          // control pipe reprl -> child

    if (pipe(crpipe) != 0) {
        return reprl_error(ctx, "Could not create pipe for REPRL communication: %s", strerror(errno));
    }
    if (pipe(cwpipe) != 0) {
        close(crpipe[0]);
        close(crpipe[1]);
        return reprl_error(ctx, "Could not create pipe for REPRL communication: %s", strerror(errno));
    }

    ctx->ctrl_in = crpipe[0];
    ctx->ctrl_out = cwpipe[1];
    fcntl(ctx->ctrl_in, F_SETFD, FD_CLOEXEC);
    fcntl(ctx->ctrl_out, F_SETFD, FD_CLOEXEC);

    int pid = fork();
    if (pid == 0) {
        // fprintf(stderr, "Child: Setting up file descriptors\n");
        // fprintf(stderr, "Child: cwpipe[0]=%d -> REPRL_CHILD_CTRL_IN=%d\n", cwpipe[0], REPRL_CHILD_CTRL_IN);
        // fprintf(stderr, "Child: crpipe[1]=%d -> REPRL_CHILD_CTRL_OUT=%d\n", crpipe[1], REPRL_CHILD_CTRL_OUT);
        // fprintf(stderr, "Child: data_out->fd=%d -> REPRL_CHILD_DATA_IN=%d\n", ctx->data_out->fd, REPRL_CHILD_DATA_IN);
        // fprintf(stderr, "Child: data_in->fd=%d -> REPRL_CHILD_DATA_OUT=%d\n", ctx->data_in->fd, REPRL_CHILD_DATA_OUT);
        
        if (dup2(cwpipe[0], REPRL_CHILD_CTRL_IN) < 0 ||
            dup2(crpipe[1], REPRL_CHILD_CTRL_OUT) < 0 ||
            dup2(ctx->data_out->fd, REPRL_CHILD_DATA_IN) < 0 ||
            dup2(ctx->data_in->fd, REPRL_CHILD_DATA_OUT) < 0) {
            fprintf(stderr, "dup2 failed in the child: %s\n", strerror(errno));
            _exit(-1);
        }

        close(cwpipe[0]);
        close(crpipe[1]);

        int devnull = open("/dev/null", O_RDWR);
        dup2(devnull, 0);

		// The following lines can be commented out to see the stdout/stderr of the JS engine in the main console (for debugging)
        if ( getenv("DOUTPUT") == NULL) {
            if (ctx->stdout) dup2(ctx->stdout->fd, 1);
            else dup2(devnull, 1);
            if (ctx->stderr) dup2(ctx->stderr->fd, 2);
            else dup2(devnull, 2);
        }
        close(devnull);

        // Debug: Check current state of REPRL fds before execve
        // fprintf(stderr, "Child: Checking REPRL fds before execve:\n");
        // fprintf(stderr, "  fd %d (CTRL_IN): %s\n", REPRL_CHILD_CTRL_IN, fcntl(REPRL_CHILD_CTRL_IN, F_GETFD) >= 0 ? "open" : "closed");
        // fprintf(stderr, "  fd %d (CTRL_OUT): %s\n", REPRL_CHILD_CTRL_OUT, fcntl(REPRL_CHILD_CTRL_OUT, F_GETFD) >= 0 ? "open" : "closed");
        // fprintf(stderr, "  fd %d (DATA_IN): %s\n", REPRL_CHILD_DATA_IN, fcntl(REPRL_CHILD_DATA_IN, F_GETFD) >= 0 ? "open" : "closed");
        // fprintf(stderr, "  fd %d (DATA_OUT): %s\n", REPRL_CHILD_DATA_OUT, fcntl(REPRL_CHILD_DATA_OUT, F_GETFD) >= 0 ? "open" : "closed");
        
        // close all other FDs. We try to use FD_CLOEXEC everywhere, but let's be extra sure we don't leak any fds to the child.
        int tablesize = getdtablesize();
        for (int i = 3; i < tablesize; i++) {
            if (i == REPRL_CHILD_CTRL_IN || i == REPRL_CHILD_CTRL_OUT || i == REPRL_CHILD_DATA_IN || i == REPRL_CHILD_DATA_OUT) {
                continue;
            }
            close(i);
        }
        execve(ctx->argv[0], ctx->argv, ctx->envp);

        fprintf(stderr, "Failed to execute child process %s: %s\n", ctx->argv[0], strerror(errno));
        fflush(stderr);
        _exit(-1);
    }

    close(crpipe[1]);
    close(cwpipe[0]);

    if (pid < 0) {
        close(ctx->ctrl_in);
        close(ctx->ctrl_out);
        return reprl_error(ctx, "Failed to fork: %s", strerror(errno));
    }
    ctx->pid = pid;
    
    // Give the child a moment to initialize
    usleep(10000); // 10ms
    
    char helo[5] = { 0 };
    // fprintf(stderr, "Parent: Waiting for HELO from child on fd %d\n", ctx->ctrl_in);
    ssize_t n = read(ctx->ctrl_in, helo, 4);
    if (n != 4) {
        // fprintf(stderr, "Parent: Failed to read HELO, got %zd bytes: %s\n", n, strerror(errno));
        reprl_terminate_child(ctx);
        return reprl_error(ctx, "Did not receive HELO message from child: %s", strerror(errno));
    }
    // fprintf(stderr, "Parent: Received: %c%c%c%c\n", helo[0], helo[1], helo[2], helo[3]);
    if (strncmp(helo, "HELO", 4) != 0) {
        reprl_terminate_child(ctx);
        return reprl_error(ctx, "Received invalid HELO message from child: %s", helo);
    }

    // fprintf(stderr, "Parent: Sending HELO reply on fd %d\n", ctx->ctrl_out);
    if (write(ctx->ctrl_out, helo, 4) != 4) {
        reprl_terminate_child(ctx);
        return reprl_error(ctx, "Failed to send HELO reply message to child: %s", strerror(errno));
    }
    // fprintf(stderr, "Parent: HELO handshake complete\n");

    return 0;
}



struct reprl_context* reprl_create_context()
{
    // "Reserve" the well-known REPRL fds so no other fd collides with them.
    // This would cause various kinds of issues in reprl_spawn_child.
    // It would be enough to do this once per process in the case of multiple
    // REPRL instances, but it's probably not worth the implementation effort.
    
    // Only reserve these in the parent process, not in the fuzzer process itself
    static int reserved = 0;
    if (!reserved) {
        reserved = 1;
        int devnull = open("/dev/null", O_RDWR);
        if (devnull >= 0) {
            dup2(devnull, REPRL_CHILD_CTRL_IN);
            dup2(devnull, REPRL_CHILD_CTRL_OUT);
            dup2(devnull, REPRL_CHILD_DATA_IN);
            dup2(devnull, REPRL_CHILD_DATA_OUT);
            close(devnull);
        }
    }

    return calloc(1, sizeof(struct reprl_context));
}

int reprl_initialize_context(struct reprl_context* ctx, char** argv, char** envp, int capture_stdout, int capture_stderr, int worker_id)
{
    if (ctx->initialized) {
        return reprl_error(ctx, "Context is already initialized");
    }

    // We need to ignore SIGPIPE since we could end up writing to a pipe after our child process has exited.
    signal(SIGPIPE, SIG_IGN);

	ctx->argv = argv;
	ctx->envp = envp;
    //ctx->argv = copy_string_array(argv);
    //ctx->envp = copy_string_array(envp);

    ctx->data_in = reprl_create_data_channel(ctx, worker_id);
    ctx->data_out = reprl_create_data_channel(ctx, worker_id);
    if (capture_stdout) {
        ctx->stdout = reprl_create_data_channel(ctx, worker_id);
    }
    if (capture_stderr) {
        ctx->stderr = reprl_create_data_channel(ctx, worker_id);
    }
    if (!ctx->data_in || !ctx->data_out || (capture_stdout && !ctx->stdout) || (capture_stderr && !ctx->stderr)) {
        // Proper error message will have been set by reprl_create_data_channel
        return -1;
    }

    ctx->initialized = 1;
    return 0;
}



void reprl_destroy_context(int worker_id)
{
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    reprl_terminate_child(current_reprl_context);

    //free_string_array(ctx->argv);
    //free_string_array(ctx->envp);

    reprl_destroy_data_channel(current_reprl_context, current_reprl_context->data_in);
    reprl_destroy_data_channel(current_reprl_context, current_reprl_context->data_out);
    reprl_destroy_data_channel(current_reprl_context, current_reprl_context->stdout);
    reprl_destroy_data_channel(current_reprl_context, current_reprl_context->stderr);

    free(current_reprl_context->last_error);
    free(current_reprl_context);
}


int reprl_execute(struct reprl_context* ctx, const char* script, uint64_t script_length, uint64_t timeout, uint64_t* execution_time, int fresh_instance, int worker_id)
{
    if (!ctx->initialized) {
        return reprl_error(ctx, "REPRL context is not initialized");
    }
    if (script_length > REPRL_MAX_DATA_SIZE) {
        return reprl_error(ctx, "Script too large");
    }

    // Terminate any existing instance if requested.
    if (fresh_instance && ctx->pid) {
        reprl_terminate_child(ctx);
    }

    // Reset file position so the child can simply read(2) and write(2) to these fds.
    lseek(ctx->data_out->fd, 0, SEEK_SET);
    lseek(ctx->data_in->fd, 0, SEEK_SET);
    if (ctx->stdout) {
        lseek(ctx->stdout->fd, 0, SEEK_SET);
    }
    if (ctx->stderr) {
        lseek(ctx->stderr->fd, 0, SEEK_SET);
    }

    // Spawn a new instance if necessary.
    if (!ctx->pid) {
        int r = reprl_spawn_child(ctx);
        if (r != 0) return r;
    }

    // Copy the script to the data channel.
    memcpy(ctx->data_out->mapping, script, script_length);
    
    // printf("reprl_execute: Sending script of length %llu to child\n", (unsigned long long)script_length);

    // Note:
    // I think resetting the current coverage map (in shared memory) here is not required because
    // the code in d8 should already reset it. However, I detected some flaws (especially when the global coverage map
    // was restored. I therefore added also the call here to just get 100% sure
    // If I later start to boost performance I can maybe remove this code again
    // (since this code will be executed in every iteration)
    // TODO: Check this
    coverage_clear_bitmap(worker_id);

    // Tell child to execute the script.
    if (write(ctx->ctrl_out, "cexe", 4) != 4 ||
        write(ctx->ctrl_out, &script_length, 8) != 8) {
        // These can fail if the child unexpectedly terminated between executions.
        // Check for that here to be able to provide a better error message.
        int status;
        if (waitpid(ctx->pid, &status, WNOHANG) == ctx->pid) {
            reprl_child_terminated(ctx);
            if (WIFEXITED(status)) {
                return reprl_error(ctx, "Child unexpectedly exited with status %i between executions", WEXITSTATUS(status));
            } else {
                return reprl_error(ctx, "Child unexpectedly terminated with signal %i between executions", WTERMSIG(status));
            }
        }
        return reprl_error(ctx, "Failed to send command to child process: %s", strerror(errno));
    }

    // Wait for child to finish execution (or crash).
    int timeout_ms = timeout / 1000;
    uint64_t start_time = current_usecs();
    struct pollfd fds = {.fd = ctx->ctrl_in, .events = POLLIN, .revents = 0};
    int res = poll(&fds, 1, timeout_ms);
    *execution_time = current_usecs() - start_time;
    if (res == 0) {
        // Execution timed out. Kill child and return a timeout status.
        reprl_terminate_child(ctx);
        return 1 << 16;
    } else if (res != 1) {
        // An error occurred.
        // We expect all signal handlers to be installed with SA_RESTART, so receiving EINTR here is unexpected and thus also an error.
        return reprl_error(ctx, "Failed to poll: %s", strerror(errno));
    }

    // Poll succeeded, so there must be something to read now (either the status or EOF).
    int status;
    ssize_t rv = read(ctx->ctrl_in, &status, 4);
    // printf("Read status: in worker %d: %d -> return status: %d\n", worker_id, status, rv);
    if (rv < 0) {
        return reprl_error(ctx, "Failed to read from control pipe: %s", strerror(errno));
    } else if (rv != 4) {
        // Most likely, the child process crashed and closed the write end of the control pipe.
        // Unfortunately, there probably is nothing that guarantees that waitpid() will immediately succeed now,
        // and we also don't want to block here. So just retry waitpid() a few times...
        int success = 0;
        do {
            success = waitpid(ctx->pid, &status, WNOHANG) == ctx->pid;
            if (!success) usleep(10);
        } while (!success && current_usecs() - start_time < timeout);

        if (!success) {
            // Wait failed, so something weird must have happened. Maybe somehow the control pipe was closed without the child exiting?
            // Probably the best we can do is kill the child and return an error.
            reprl_terminate_child(ctx);
            return reprl_error(ctx, "Child in weird state after execution");
        }

        // Cleanup any state related to this child process.
        reprl_child_terminated(ctx);

        if (WIFEXITED(status)) {
            status = WEXITSTATUS(status) << 8;
        } else if (WIFSIGNALED(status)) {
            status = WTERMSIG(status);
        } else {
            // This shouldn't happen, since we don't specify WUNTRACED for waitpid...
            return reprl_error(ctx, "Waitpid returned unexpected child state %i", status);
        }
    }

    // The status must be a positive number, see the status encoding format below.
    // We also don't allow the child process to indicate a timeout. If we wanted,
    // we could treat it as an error if the upper bits are set.
    status &= 0xffff;

    return status;
}

// Sets the coverage back to zero (should be called before every execution)
void coverage_clear_bitmap(int worker_id) {
    struct cov_context* context = &contexts[worker_id];
    if (context != NULL) {
        memset(context->shmem->edges, 0, context->bitmap_size);
    }
    else{
        printf("Context is NULL for worker %d\n", worker_id);
    }
}

int cov_get_edge_counts(struct cov_context* context, struct edge_counts* edges)
{
    if(!context->should_track_edges) {
        return -1;
    }
    edges->edge_hit_count = context->edge_count;
    edges->count = context->num_edges;
    return 0;
}

void cov_clear_edge_data(int worker_id, uint32_t index)
{
    struct cov_context* context = &contexts[worker_id];
    if (context->should_track_edges) {
        assert(context->edge_count[index]);
        context->edge_count[index] = 0;
    }
    context->found_edges -= 1;
    // assert(!edge(context->virgin_bits, index));
    set_edge(context->virgin_bits, index);
}
void cov_set_edge_data(int worker_id, uint32_t index)
{
    struct cov_context* context = &contexts[worker_id];
    if (context->should_track_edges) {
        assert(context->edge_count[index] == 0);
        context->edge_count[index] = 1;
    }
    context->found_edges += 1;
    // assert(!edge(context->virgin_bits, index));
    clear_edge(context->virgin_bits, index);
}


void cov_reset_state(int worker_id) {
    struct cov_context* context = &contexts[worker_id];
    memset(context->virgin_bits, 0xff, context->bitmap_size);
    memset(context->crash_bits, 0xff, context->bitmap_size);

    if (context->edge_count != NULL) {
        memset(context->edge_count, 0, sizeof(uint32_t) * context->num_edges);
    }

    // Zeroth edge is ignored, see above.
    clear_edge(context->virgin_bits, 0);
    clear_edge(context->crash_bits, 0);

    context->found_edges = 0;
}


static char* fetch_data_channel_content(struct data_channel* channel) {
    if (!channel) return "";
    
    // Get the actual size of data written to the channel
    off_t current_pos = lseek(channel->fd, 0, SEEK_CUR);
    off_t file_size = lseek(channel->fd, 0, SEEK_END);
    
    // Restore the original position
    lseek(channel->fd, current_pos, SEEK_SET);
    
    // Ensure we don't exceed the mapping size
    size_t content_size = MIN(file_size, REPRL_MAX_DATA_SIZE - 1);
    
    // Null-terminate the content
    channel->mapping[content_size] = 0;
    
    return channel->mapping;
}

char* reprl_fetch_fuzzout(int worker_id) {
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    return fetch_data_channel_content(current_reprl_context->data_in);
}

char* reprl_fetch_stdout(int worker_id) {
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    return fetch_data_channel_content(current_reprl_context->stdout);
}

char* reprl_fetch_stderr(int worker_id) {
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    return fetch_data_channel_content(current_reprl_context->stderr);
}

char* reprl_get_last_error(int worker_id) {
    struct reprl_context* current_reprl_context = reprl_contexts[worker_id];
    // last_error is a char*, not a data_channel*
    // For now, return an empty string
    return current_reprl_context->last_error ? current_reprl_context->last_error : "";
}
