#include "backtracking.h"
#include "backtracking_addon_host.h"
#include <stdio.h>
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    BtCore *core = bt_new(48000, 2, 10);
    BtAddonHost *host = bt_addon_host_new(argv[1], "0.2.1", BT_ADDON_PRIMARY);
    if (!core || !host) return 1;
    char *snapshot = bt_addon_host_snapshot(host);
    puts(snapshot);
    bt_addon_string_free(snapshot);
    bt_addon_host_free(host);
    bt_free(core);
}
