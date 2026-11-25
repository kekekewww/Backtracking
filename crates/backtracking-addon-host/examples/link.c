#include "backtracking.h"
#include "backtracking_addon_host.h"
#include "backtracking_positions.h"
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
struct Waiter {
    BtAddonHost *host;
    bool done;
};
static void *wait_for_shutdown(void *arg) {
    struct Waiter *waiter = arg;
    waiter->done = bt_addon_host_shutdown(waiter->host, 2500);
    return NULL;
}
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
    const char *requests[] = {
        "{\"schema\":1,\"op\":\"take_control\"}",
        "{\"schema\":1,\"op\":\"positions_since\",\"cursor\":0}",
        "{\"schema\":1,\"op\":\"control_reply\",\"addon_id\":\"missing\",\"run\":1,"
        "\"request_id\":\"late\",\"error\":{\"code\":\"busy\",\"message\":\"test\"}}",
        "{\"schema\":1,\"op\":\"control_reply\",\"addon_id\":\"missing\",\"run\":1,\"request_id\":\"bad\"}",
        "{\"schema\":1,\"op\":\"take_result\",\"ticket\":999}"
    };
    const char *expected[] = {
        "\"value\":[]", "\"positions\":[]", "\"value\":null", "\"code\":\"invalid_request\"", "\"code\":\"not_found\""
    };
    uint64_t generation = bt_addon_host_generation(host);
    int ok = ready;
    for (int poll = 0; poll < 150; ++poll) {
        for (size_t i = 0; i < sizeof(requests)/sizeof(requests[0]); ++i) {
            char *reply = bt_addon_host_command(host, requests[i]);
            if (!strstr(reply, expected[i]) || strstr(reply, "\"ticket\":")) ok = 0;
            if (!poll) puts(reply);
            bt_addon_string_free(reply);
        }
    }
    char *snapshot = bt_addon_host_snapshot(host);
    ok = ok && strstr(snapshot, "\"completed\":[]") && generation == bt_addon_host_generation(host);
    bt_addon_string_free(snapshot);

    BtPositionTrack *positions = bt_positions_new();
    bt_positions_insert(positions, 0, (BtPositionKey){.frame=1, .gain=1});
    bt_positions_clock_evicted_before(positions, 2);
    BtFrozenPositions *frozen = bt_positions_freeze(positions);
    ok = ok && bt_positions_take_overflowed(positions)
            && bt_frozen_positions_read(frozen, 0, 0, 10, NULL, 0) == 0;
    bt_frozen_positions_free(frozen);
    bt_positions_free(positions);
    struct Waiter other = {.host=host};
    pthread_t thread;
    if (pthread_create(&thread, NULL, wait_for_shutdown, &other) != 0) return 1;
    bool shutdown = bt_addon_host_shutdown(host, 2500);
    if (pthread_join(thread, NULL) != 0) return 1;
    bool repeated = bt_addon_host_shutdown(host, 0);
    bool missing = bt_addon_host_shutdown(NULL, 2500);
    printf("shutdown=%d concurrent=%d repeated=%d null=%d\n", shutdown, other.done, repeated, missing);
    ok = ok && shutdown && other.done && repeated && !missing;
    bt_addon_host_free(host);
    bt_free(core);
    return ok ? 0 : 1;
}
