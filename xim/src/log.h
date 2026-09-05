/*
 * 日志:~/.local/share/lyyime/logs/xim.log(AGENTS.MD 边界完善红线)
 * 格式:YYYY-MM-DD HH:MM:SS [级别] 消息;追加写,启动时打横幅。
 */
#ifndef LYY_LOG_H_
#define LYY_LOG_H_

#include <stdio.h>

typedef struct {
    FILE *fp;
    char path[1024];
} LyyLog;

/* 打开日志文件(目录不存在则递归创建);失败时退回 stderr */
void lyy_log_open(LyyLog *log, const char *path);

/* 写一行日志(printf 风格);log 为 NULL 时写到 stderr,便于极早期排障 */
void lyy_log(LyyLog *log, const char *fmt, ...)
#ifdef __GNUC__
    __attribute__((format(printf, 2, 3)))
#endif
    ;

void lyy_log_close(LyyLog *log);

#endif /* LYY_LOG_H_ */
