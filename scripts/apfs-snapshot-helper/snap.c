/* snap.c — create a persistent APFS snapshot on a mounted volume via the
 * fs_snapshot_create(2) private syscall (Apple <sys/snapshot.h>). Used by
 * gen-images to build the apfs-snapshots golden image (docs/plan/04 §3.1 step
 * 6); no third-party code. Usage: snap <mountpoint> <snapshot-name> */
#include <sys/attr.h>
#include <sys/snapshot.h>
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) { fprintf(stderr, "usage: snap <mount> <name>\n"); return 2; }
    int fd = open(argv[1], O_RDONLY);
    if (fd < 0) { perror("open"); return 1; }
    int rc = fs_snapshot_create(fd, argv[2], 0);
    if (rc != 0) { perror("fs_snapshot_create"); close(fd); return 1; }
    close(fd);
    printf("created snapshot %s on %s\n", argv[2], argv[1]);
    return 0;
}
