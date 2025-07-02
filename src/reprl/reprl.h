


#include <stdint.h>
#include <limits.h>
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


// Tracks a set of edges by their indices
struct edge_set {
    uint32_t count;
    uint32_t * edge_indices;
};

// Tracks the hit count of all edges
struct edge_counts {
    uint32_t count;
    uint32_t * edge_hit_count;
};


// static const int kMaxCmpEvents = 1024000; // 1M
// #define SHM_SIZE (0x1000000  + kMaxCmpEvents * 16) // 16MB

// #define MAX_EDGES ((0x1000000 - 4) * 8)

// struct shmem_data {
//   uint32_t num_edges;
//   uint64_t event_count;
//   struct CmpEvent g_cmp_events[1024000]; 
//   unsigned char edges[];
  
// };


#define SHM_SIZE 0x100000
#define MAX_EDGES ((SHM_SIZE - 4) * 8)

struct shmem_data {
  uint32_t num_edges;
  unsigned char edges[];
};



struct cov_context {
    // Id of this coverage context.
    int id;

    int should_track_edges;

    // Bitmap of edges that have been discovered so far.
    uint8_t* virgin_bits;

    uint8_t* virgin_bits_backup;
    // Bitmap of edges that have been discovered in crashing samples so far.
    uint8_t* crash_bits;

    // Total number of edges in the target program.
    uint32_t num_edges;
    
    
    uint8_t* coverage_map_backup;	// This is used to backup the result coverage map of one execution (if new coverage is found)


    // Number of used bytes in the shmem->edges bitmap, roughly num_edges / 8.
    uint32_t bitmap_size;

    // Total number of edges that have been discovered so far.
    uint32_t found_edges;


    // Pointer into the shared memory region.
    struct shmem_data* shmem;

    // Count of occurrences per edge
    uint32_t * edge_count;
};

/// Maximum size for data transferred through REPRL. In particular, this is the maximum size of scripts that can be executed.
/// Currently, this is 16MB. Executing a 16MB script file is very likely to take longer than the typical timeout, so the limit on script size shouldn't be a problem in practice.
#define REPRL_MAX_DATA_SIZE (16 << 20)

/// Opaque struct representing a REPRL execution context.

// A unidirectional communication channel for larger amounts of data, up to a maximum size (REPRL_MAX_DATA_SIZE).
// Implemented as a (RAM-backed) file for which the file descriptor is shared with the child process and which is mapped into our address space.
struct data_channel {
    // File descriptor of the underlying file. Directly shared with the child process.
    int fd;
    // Memory mapping of the file, always of size REPRL_MAX_DATA_SIZE.
    char* mapping;
};

struct reprl_context {
    // Whether reprl_initialize has been successfully performed on this context.
    int initialized;

    // Read file descriptor of the control pipe. Only valid if a child process is running (i.e. pid is nonzero).
    int ctrl_in;
    // Write file descriptor of the control pipe. Only valid if a child process is running (i.e. pid is nonzero).
    int ctrl_out;

    // Data channel REPRL -> Child
    struct data_channel* data_in;
    // Data channel Child -> REPRL
    struct data_channel* data_out;

    // Optional data channel for the child's stdout and stderr.
    struct data_channel* stdout;
    struct data_channel* stderr;

    // PID of the child process. Will be zero if no child process is currently running.
    int pid;

    // Arguments and environment for the child process.
    char** argv;
    char** envp;

    // A malloc'd string containing a description of the last error that occurred.
    char* last_error;
};


/// Allocates a new REPRL context.
/// @return an uninitialzed REPRL context
struct reprl_context* reprl_create_context();

/// Initializes a REPRL context.
///
/// @param ctx An uninitialized context
/// @param argv The argv vector for the child processes
/// @param envp The envp vector for the child processes
/// @param capture_stdout Whether this REPRL context should capture the child's stdout
/// @param capture_stderr Whether this REPRL context should capture the child's stderr
/// @return zero in case of no errors, otherwise a negative value
int reprl_initialize_context(struct reprl_context* ctx, char** argv, char** envp, int capture_stdout, int capture_stderr, int worker_id);

void init(int worker_id);
void spawn(int worker_id);

void coverage_clear_bitmap(int worker_id);
uint32_t coverage_finish_initialization(int worker_id, int should_track_edges);
int cov_evaluate(int worker_id,struct edge_set* new_edges);
struct CmpEvent* cov_fetch_cmp_events(int worker_id);
uint64_t fetch_event_count(int worker_id);
void cov_clear_cmp_events(int worker_id);
int execute_script(char* arg_script_string, int arg_timeout, int fresh_instance, int worker_id);


/// Destroys a REPRL context, freeing all resources held by it.
///
/// @param ctx The context to destroy
// void reprl_destroy_context(struct reprl_context* ctx);
void reprl_destroy_context(int worker_id);

/// Executes the provided script in the target process, wait for its completion, and return the result.
/// If necessary, or if fresh_instance is true, this will automatically spawn a new instance of the target process.
///
/// @param ctx The REPRL context
/// @param script The script to execute
/// @param script_length The size of the script in bytes
/// @param timeout The maximum allowed execution time in microseconds
/// @param execution_time A pointer to which, if execution succeeds, the execution time in microseconds is written to
/// @param fresh_instance if true, forces the creation of a new instance of the target
/// @return A REPRL exit status (see below) or a negative number in case of an error
int reprl_execute(struct reprl_context* ctx, const char* script, uint64_t script_length, uint64_t timeout, uint64_t* execution_time, int fresh_instance, int worker_id);

int coverage_save_virgin_bits_in_file(int worker_id, const char *filepath);

int coverage_load_virgin_bits_from_file(int worker_id,const char *filepath);
/// Returns true if the execution terminated due to a signal.
///
/// The 32bit REPRL exit status as returned by reprl_execute has the following format:
///     [ 00000000 | did_timeout | exit_code | terminating_signal ]
/// Only one of did_timeout, exit_code, or terminating_signal may be set at one time.
static inline int RIFSIGNALED(int status)
{
    return (status & 0xff) != 0;
}

/// Returns true if the execution terminated due to a timeout.
static inline int RIFTIMEDOUT(int status)
{
    return (status & 0xff0000) != 0;
}

/// Returns true if the execution finished normally.
static inline int RIFEXITED(int status)
{
    return !RIFSIGNALED(status) && !RIFTIMEDOUT(status);
}

/// Returns the terminating signal in case RIFSIGNALED is true.
static inline int RTERMSIG(int status)
{
    return status & 0xff;
}

/// Returns the exit status in case RIFEXITED is true.
static inline int REXITSTATUS(int status)
{
    return (status >> 8) & 0xff;
}

/// Returns the stdout data of the last successful execution if the context is capturing stdout, otherwise an empty string.
/// The output is limited to REPRL_MAX_DATA_SIZE (currently 16MB).
///
/// @param ctx The REPRL context
/// @return A string pointer which is owned by the REPRL context and thus should not be freed by the caller
char* reprl_fetch_stdout(int worker_id);

/// Returns the stderr data of the last successful execution if the context is capturing stderr, otherwise an empty string.
/// The output is limited to REPRL_MAX_DATA_SIZE (currently 16MB).
///
/// @param ctx The REPRL context
/// @return A string pointer which is owned by the REPRL context and thus should not be freed by the caller
char* reprl_fetch_stderr(int worker_id);

/// Returns the fuzzout data of the last successful execution.
/// The output is limited to REPRL_MAX_DATA_SIZE (currently 16MB).
///
/// @param ctx The REPRL context
/// @return A string pointer which is owned by the REPRL context and thus should not be freed by the caller
char* reprl_fetch_fuzzout(int worker_id);
/// Returns a string describing the last error that occurred in the given context.
///
/// @param ctx The REPRL context
/// @return A string pointer which is owned by the REPRL context and thus should not be freed by the caller
char* reprl_get_last_error(int worker_id);



// Well-known file descriptor numbers for reprl <-> child communication, child process side
// Fuzzilli REPRL fd numbers
#define REPRL_CHILD_CTRL_IN 100   // REPRL_CRFD in Fuzzilli
#define REPRL_CHILD_CTRL_OUT 101  // REPRL_CWFD in Fuzzilli
#define REPRL_CHILD_DATA_IN 102   // REPRL_DRFD in Fuzzilli
#define REPRL_CHILD_DATA_OUT 103  // REPRL_DWFD in Fuzzilli

#define MIN(x, y) ((x) < (y) ? (x) : (y))

/// Maximum timeout in microseconds. Mostly just limited by the fact that the timeout in milliseconds has to fit into a 32-bit integer.
#define REPRL_MAX_TIMEOUT_IN_MICROSECONDS ((uint64_t)(INT_MAX) * 1000)