/* Harness-only passive executable-entry evidence. No product hooks or policy.
 * Inherited only in the private namespace; records no environment/capabilities.
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
__attribute__((constructor)) static void audit(void) {
    const char *path = getenv("AGE360_PROCESS_AUDIT");
    if (!path) return;
    char exe[4096] = {0}, argv[16384] = {0}, line[22000];
    ssize_t n = readlink("/proc/self/exe", exe, sizeof(exe)-1);
    if (n < 0) return;
    int input = open("/proc/self/cmdline", O_RDONLY);
    if (input < 0) return;
    n = read(input, argv, sizeof(argv)-1);
    close(input);
    for (ssize_t i=0; i<n; i++) if (!argv[i] || argv[i]=='\n' || argv[i]=='\t') argv[i]=' ';
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    int len = snprintf(line, sizeof(line), "%ld.%09ld\t%d\t%d\t%s\t%s\n", ts.tv_sec, ts.tv_nsec, getpid(), getppid(), exe, argv);
    int output = open(path, O_WRONLY|O_APPEND|O_CREAT, 0600);
    if (output >= 0) { if (len > 0 && len < sizeof(line)) { ssize_t written = write(output,line,len); (void)written; } close(output); }
}

/* Optional harness-local SQLite auto-extension. The unmodified executable's
 * symbol offsets are obtained from its own nm output and bound to its copied
 * hash by the Python controller. No code is patched or SQL rewritten. SQLite's
 * extension API supplies read-only tracing and an innocuous PID scalar used
 * exclusively by test audit triggers. Missing support fails the experiment.
 */
#include <dlfcn.h>
#include <stdint.h>
#include <string.h>
#include <sqlite3ext.h>
#include <openssl/evp.h>
#undef sqlite3_api
static _Atomic(const sqlite3_api_routines *) api;
static void record_sql_writer(void) {
    static _Thread_local pid_t recorded;
    if (recorded == getpid()) return;
    const char *path = getenv("AGE360_SQL_AUDIT");
    if (!path) return;
    FILE *exe = fopen("/proc/self/exe", "rb");
    EVP_MD_CTX *ctx = EVP_MD_CTX_new();
    if (!exe || !ctx || EVP_DigestInit_ex(ctx, EVP_sha256(), NULL) != 1) _exit(93);
    unsigned char buffer[65536], hash[EVP_MAX_MD_SIZE];
    size_t n;
    while ((n = fread(buffer, 1, sizeof(buffer), exe)))
        if (EVP_DigestUpdate(ctx, buffer, n) != 1) _exit(93);
    if (ferror(exe)) _exit(93);
    fclose(exe);
    unsigned len;
    if (EVP_DigestFinal_ex(ctx, hash, &len) != 1 || len != 32) _exit(93);
    EVP_MD_CTX_free(ctx);
    char hex[65], target[4096] = {0};
    for (unsigned i=0; i<len; i++) sprintf(hex+2*i, "%02x", hash[i]);
    if (readlink("/proc/self/exe", target, sizeof(target)-1) < 0) _exit(93);
    char *output = NULL;
    if (asprintf(&output, "%s.writers", path) < 0) _exit(93);
    int fd = open(output, O_WRONLY|O_APPEND|O_CREAT, 0600);
    free(output);
    if (fd < 0) _exit(93);
    dprintf(fd, "%d\t%s\t%s\n", getpid(), hex, target);
    close(fd);
    recorded = getpid();
}
static void writer_pid(sqlite3_context *ctx, int n, sqlite3_value **values) {
    (void)n; (void)values;
    record_sql_writer();
    api->result_int64(ctx, getpid());
}
static int sql_trace(unsigned kind, void *context, void *statement, void *extra) {
    (void)extra;
    const char *path = getenv("AGE360_SQL_AUDIT");
    if (!path || kind != SQLITE_TRACE_PROFILE) return 0;
    record_sql_writer();
    sqlite3 *db = context;
    const char *sql = api->sql(statement);
    if (!sql || !(strstr(sql, "completion_event") || strstr(sql, "completion_continuation_source") ||
                 !strcmp(sql, "COMMIT") || !strcmp(sql, "ROLLBACK"))) return 0;
    char *expanded = api->expanded_sql(statement);
    if (!expanded) return 0;
    for (char *s = expanded; *s; s++) if (*s == '\n' || *s == '\r' || *s == '\t') *s = ' ';
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    char *line = NULL;
    int len = asprintf(&line, "%ld.%09ld\t%d\t%p\t%p\t%d\t%d\t%s\n", ts.tv_sec, ts.tv_nsec,
                       getpid(), (void *)db, statement, api->errcode(db), api->get_autocommit(db), expanded);
    int fd = open(path, O_WRONLY|O_APPEND|O_CREAT, 0600);
    if (fd >= 0) { if (len > 0) { ssize_t result = write(fd, line, len); (void)result; } close(fd); }
    free(line);
    api->free(expanded);
    return 0;
}
static int sql_extension(sqlite3 *db, char **error, const sqlite3_api_routines *routines) {
    (void)error;
    api = routines;
    int rc = api->create_function_v2(db, "e2e_writer_pid", 0, SQLITE_UTF8|SQLITE_INNOCUOUS,
                                    NULL, writer_pid, NULL, NULL, NULL);
    if (rc) return rc;
    return api->trace_v2(db, SQLITE_TRACE_PROFILE, sql_trace, db);
}
__attribute__((constructor)) static void install_sql_audit(void) {
    const char *map = getenv("AGE360_SQLITE_SYMBOLS");
    if (!map || !getenv("AGE360_SQL_AUDIT")) return;
    void *opened = dlsym(RTLD_DEFAULT, "sqlite3_open");
    Dl_info info;
    if (!opened || !dladdr(opened, &info)) return;
    FILE *file = fopen(map, "r");
    if (!file) _exit(91);
    unsigned long open_offset, extension_offset;
    while (fscanf(file, "%lx %lx", &open_offset, &extension_offset) == 2) {
        if ((uintptr_t)opened - (uintptr_t)info.dli_fbase != open_offset) continue;
        int (*register_extension)(void (*)(void)) = (void *)((uintptr_t)info.dli_fbase + extension_offset);
        if (register_extension((void (*)(void))sql_extension)) _exit(92);
        fclose(file);
        return;
    }
    fclose(file);
    /* A non-runner (e.g. Python) may load a different SQLite, never hook it. */
}
