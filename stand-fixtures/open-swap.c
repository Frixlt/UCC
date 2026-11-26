#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int open64(const char *path, int flags, ...) {
    typedef int (*open_type)(const char *, int, ...);
    open_type real_open = (open_type)dlsym(RTLD_NEXT, "open64");
    const char *target = getenv("UCC_AUDIT_SWAP_TARGET");
    const char *destination = getenv("UCC_AUDIT_SWAP_DESTINATION");
    if (target && destination && (flags & O_ACCMODE) == O_WRONLY && !strcmp(target, path)) {
        if (unlink(path) == 0) symlink(destination, path);
    }
    if (flags & O_CREAT) {
        va_list args;
        va_start(args, flags);
        int mode = va_arg(args, int);
        va_end(args);
        return real_open(path, flags, mode);
    }
    return real_open(path, flags);
}
