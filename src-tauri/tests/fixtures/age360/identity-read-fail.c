#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
/* Private explicit preload, armed only after the adopter's WNOWAIT barrier.
 * Only the selected original caller's read of the exact child's stat fails.
 * Other identity readers, descriptor reads and all writes remain real. */
static int selected_read(int fd) {
    const char *root = getenv("AGE360_IDENTITY_ROOT");
    if (!root) return 0;
    char path[4096], bytes[128], proc[64], actual[128], expected[64];
    snprintf(path, sizeof(path), "%s/identity-read-target", root);
    int arm = syscall(SYS_openat, AT_FDCWD, path, O_RDONLY, 0);
    if (arm < 0) return 0;
    ssize_t n = syscall(SYS_read, arm, bytes, sizeof(bytes)-1);
    syscall(SYS_close, arm);
    if (n <= 0) return 0;
    bytes[n] = 0;
    long caller = 0, child = 0;
    if (sscanf(bytes, "%ld %ld", &caller, &child) != 2 || caller != (long)getpid()) return 0;
    snprintf(proc, sizeof(proc), "/proc/self/fd/%d", fd);
    n = readlink(proc, actual, sizeof(actual)-1);
    if (n < 0) return 0;
    actual[n] = 0;
    snprintf(expected, sizeof(expected), "/proc/%ld/stat", child);
    if (strcmp(actual, expected)) return 0;
    snprintf(path, sizeof(path), "%s/identity-read-errors", root);
    int out = syscall(SYS_openat, AT_FDCWD, path, O_WRONLY|O_CREAT|O_APPEND, 0600);
    if (out < 0) _exit(94);
    dprintf(out, "caller=%ld path=%s read=EIO\n", caller, actual);
    syscall(SYS_close, out);
    return 1;
}
ssize_t read(int fd, void *buf, size_t count) {
    ssize_t (*real_read)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    if (selected_read(fd)) { errno = EIO; return -1; }
    return real_read(fd, buf, count);
}
