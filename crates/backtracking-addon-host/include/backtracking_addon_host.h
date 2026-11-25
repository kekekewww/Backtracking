#ifndef BACKTRACKING_ADDON_HOST_H
#define BACKTRACKING_ADDON_HOST_H
#include <stdint.h>
#include <stdbool.h>
#ifdef __cplusplus
extern "C" {
#endif
typedef struct BtAddonHost BtAddonHost;
enum { BT_ADDON_PRIMARY = 0, BT_ADDON_SECONDARY = 1 };
/* Strings are NUL-terminated UTF-8. NULL on invalid arguments.
   Startup is asynchronous; inspect snapshot.ready and snapshot.error. */
BtAddonHost *bt_addon_host_new(const char *absolute_root, const char *app_version, uint32_t role);
uint64_t bt_addon_host_generation(const BtAddonHost *host);
char *bt_addon_host_snapshot(const BtAddonHost *host);
char *bt_addon_host_command(const BtAddonHost *host, const char *command_json);
/* Pass each non-null returned string exactly once. */
void bt_addon_string_free(char *value);
/* Start shutdown once. Wait up to timeout_ms, or poll with zero. True when
   cleanup has finished, false on timeout or NULL. Safe to call again.
   Shutdown leaves the handle allocated. */
bool bt_addon_host_shutdown(BtAddonHost *host, uint32_t timeout_ms);
/* Release once, after all concurrent calls have returned. Starts background
   shutdown if needed and returns without waiting. NULL is accepted. */
void bt_addon_host_free(BtAddonHost *host);
#ifdef __cplusplus
}
#endif
#endif
