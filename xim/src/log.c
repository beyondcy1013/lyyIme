#include "log.h"

#include <stdarg.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>
#include <string.h>
#include <errno.h>

/* 递归建目录(简化版:逐级 mkdir,忽略已存在) */
static void mkdirs_for_file(const char *filepath)
{
    char buf[1024];
    snprintf(buf, sizeof(buf), "%s", filepath);
    char *p = buf;
    while ((p = strchr(p + 1, '/')) != NULL) {
        *p = '\0';
        if (mkdir(buf, 0755) != 0 && errno != EEXIST)
            return;
        *p = '/';
    }
}

void lyy_log_open(LyyLog *log, const char *path)
{
    log->fp = NULL;
    snprintf(log->path, sizeof(log->path), "%s", path);
    mkdirs_for_file(path);
    log->fp = fopen(path, "a");
}

void lyy_log(LyyLog *log, const char *fmt, ...)
{
    FILE *out = (log && log->fp) ? log->fp : stderr;

    struct timeval tv;
    gettimeofday(&tv, NULL);
    struct tm tmv;
    localtime_r(&tv.tv_sec, &tmv);
    fprintf(out, "%04d-%02d-%02d %02d:%02d:%02d.%03d ",
            tmv.tm_year + 1900, tmv.tm_mon + 1, tmv.tm_mday,
            tmv.tm_hour, tmv.tm_min, tmv.tm_sec, (int)(tv.tv_usec / 1000));

    va_list ap;
    va_start(ap, fmt);
    vfprintf(out, fmt, ap);
    va_end(ap);
    fputc('\n', out);
    fflush(out);
}

void lyy_log_close(LyyLog *log)
{
    if (log && log->fp) {
        fclose(log->fp);
        log->fp = NULL;
    }
}
