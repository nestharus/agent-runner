#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>
#include <stdint.h>
#include <errno.h>
/* Private, explicit LD_PRELOAD for the experimental SQLite writer only.
 * Keep the real fsync result and stop after it, before SQLite publishes wal-index. */
static int hold(int fd) {
  char proc[64], path[4096], marker[4096];
  const char *root=getenv("AGE360_WAL_ROOT");
  if (!root) return 0;
  snprintf(proc,sizeof(proc),"/proc/self/fd/%d",fd);
  ssize_t n=readlink(proc,path,sizeof(path)-1);
  if(n<4) return 0;
  path[n]=0;
  if(strcmp(path+n-4,"-wal")) return 0;
  /* WAL creation syncs a header before any transaction frames. Only hold a
   * genuine commit-marked frame sync, not that earlier nonpublication event. */
  unsigned char header[32], frame[8]; struct stat st;
  if(fstat(fd,&st) || pread(fd,header,32,0)!=32) return 0;
  uint32_t page=((uint32_t)header[8]<<24)|((uint32_t)header[9]<<16)|((uint32_t)header[10]<<8)|header[11];
  if(page==1) page=65536;
  if(!page || st.st_size<32+(off_t)page+24) return 0;
  off_t count=(st.st_size-32)/(page+24);
  if(pread(fd,frame,8,32+(count-1)*(page+24))!=8) return 0;
  if(!(frame[4]|frame[5]|frame[6]|frame[7])) return 0;

  snprintf(marker,sizeof(marker),"%s/armed",root);
  FILE *arm=fopen(marker,"r"); long target=0;
  if(!arm) return 0;
  int parsed=fscanf(arm,"%ld",&target); fclose(arm);
  if(parsed!=1 || target!=(long)getpid()) return 0;
  unlink(marker); /* one exact writer, not a sweep */
  snprintf(marker,sizeof(marker),"%s/synced",root);
  int out=open(marker,O_WRONLY|O_CREAT|O_EXCL,0600);
  if(out<0) _exit(92);
  dprintf(out,"%ld %s\n",(long)getpid(),path); close(out);
  snprintf(marker,sizeof(marker),"%s/release",root);
  for(int i=0;i<3000;i++) { if(!access(marker,F_OK)) {
    snprintf(marker,sizeof(marker),"%s/fail-sync",root);
    return !access(marker,F_OK);
  } usleep(10000); }
  _exit(93); /* experiment timeout is failure, never success */
}
int fsync(int fd) { int (*real)(int)=dlsym(RTLD_NEXT,"fsync"); int rc=real(fd); if(!rc && hold(fd)) { errno=EIO; return -1; } return rc; }
int fdatasync(int fd) { int (*real)(int)=dlsym(RTLD_NEXT,"fdatasync"); int rc=real(fd); if(!rc && hold(fd)) { errno=EIO; return -1; } return rc; }
