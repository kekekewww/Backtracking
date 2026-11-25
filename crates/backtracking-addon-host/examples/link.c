#include "backtracking.h"
#include "backtracking_addon_host.h"
#include "backtracking_positions.h"
#include <stdio.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    BtCore *core = bt_new(48000, 2, 10);
    BtAddonHost *host = bt_addon_host_new(argv[1], "0.2.1", BT_ADDON_PRIMARY);
    if (!core || !host || !bt_capture_clock_ns()) return 1;
    int ready = 0;
    for (int i = 0; i < 500 && !ready; ++i) {
        char *snapshot = bt_addon_host_snapshot(host);
        ready = strstr(snapshot, "\"ready\":true") != NULL;
        if (ready) puts(snapshot);
        bt_addon_string_free(snapshot);
        if (!ready) usleep(10000);
    }
    uint64_t generation = bt_addon_host_generation(host);
    char *calls = bt_addon_host_command(host, "{\"schema\":1,\"op\":\"take_control\"}");
    int ok = ready && strstr(calls, "\"value\":[]") && generation == bt_addon_host_generation(host);
    puts(calls);
    bt_addon_string_free(calls);
    bt_addon_host_free(host);
    bt_free(core);
    return ok ? 0 : 1;
}
