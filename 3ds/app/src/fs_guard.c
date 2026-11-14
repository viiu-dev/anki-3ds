// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

#include <errno.h>
#include <sys/iosupport.h>
#include <sys/stat.h>

int __real_mkdir(const char *path, mode_t mode);

int __wrap_mkdir(const char *path, mode_t mode) {
    if (!path || !path[0]) {
        errno = ENOENT;
        return -1;
    }
    int dev = FindDevice(path);
    if (dev < 0 || dev >= STD_MAX || !devoptab_list[dev] || !devoptab_list[dev]->mkdir_r) {
        errno = ENODEV;
        return -1;
    }
    return __real_mkdir(path, mode);
}
