/* Diagnostic main-thread PC sampling without perf privileges or signal-time I/O. */
#define _GNU_SOURCE
#include <fcntl.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <time.h>
#include <ucontext.h>
#include <unistd.h>

enum { CAPACITY = 8192 };
typedef struct { uint64_t time, pc; } Sample;
typedef struct { _Atomic uint64_t count; Sample samples[CAPACITY]; } Samples;
static Samples *samples;

static void
sample(int signal, siginfo_t *info, void *context)
{
	(void)signal; (void)info;
	uint64_t index = atomic_load_explicit(&samples->count, memory_order_relaxed);
	if (index == CAPACITY) return;
	ucontext_t *state = context;
#if defined(__x86_64__)
	uint64_t pc = state->uc_mcontext.gregs[REG_RIP];
#elif defined(__aarch64__)
	uint64_t pc = state->uc_mcontext.pc;
#else
#error Unsupported diagnostic sampling architecture
#endif
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	samples->samples[index] = (Sample){(uint64_t)now.tv_sec * 1000000000 + now.tv_nsec, pc};
	atomic_store_explicit(&samples->count, index + 1, memory_order_release);
}

__attribute__((constructor)) static void
start(void)
{
	const char *directory = getenv("WORMINAL_BROWSER_CPU_DIR");
	if (!directory) return;
	char executable[4096];
	ssize_t length = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
	if (length < 0) return;
	executable[length] = '\0';
	const char *name = strrchr(executable, '/');
	name = name ? name + 1 : executable;
	if (strcmp(name, "worminal") && strcmp(name, "WebKitWebProcess") &&
	    strcmp(name, "WebKitNetworkProcess")) return;
	char path[4096];
	if (snprintf(path, sizeof(path), "%s/%ld.samples", directory, (long)getpid()) >= (int)sizeof(path)) return;
	int fd = open(path, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (fd < 0) return;
	if (ftruncate(fd, sizeof(Samples)) < 0) { close(fd); return; }
	samples = mmap(NULL, sizeof(Samples), PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	close(fd);
	if (samples == MAP_FAILED) { samples = NULL; return; }
	struct sigaction action = {.sa_sigaction = sample, .sa_flags = SA_SIGINFO | SA_RESTART};
	sigemptyset(&action.sa_mask);
	if (sigaction(SIGPROF, &action, NULL)) return;
	struct sigevent event = {.sigev_notify = SIGEV_THREAD_ID, .sigev_signo = SIGPROF};
	event._sigev_un._tid = syscall(SYS_gettid);
	timer_t timer;
	if (timer_create(CLOCK_THREAD_CPUTIME_ID, &event, &timer)) return;
	struct itimerspec interval = {.it_interval = {.tv_nsec = 1000000}, .it_value = {.tv_nsec = 1000000}};
	timer_settime(timer, 0, &interval, NULL);
}
