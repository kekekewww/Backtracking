#include <X11/XKBlib.h>
#include <X11/Xlib.h>
#include <X11/keysym.h>
#include <gio/gio.h>
#include <stdio.h>
#include <string.h>

typedef void (*SaveCallback)(const void *);

static int grab_error;

static int remember_x11_error(Display *display, XErrorEvent *event) {
    (void)display;
    grab_error = event->error_code;
    return 0;
}

static int physical_grave_key(Display *display) {
    XkbDescPtr keyboard = XkbGetKeyboard(display, XkbGBN_KeyNamesMask, XkbUseCoreKbd);
    if (!keyboard) return 0;

    int result = 0;
    if (keyboard->names && keyboard->names->keys) {
        for (int key = keyboard->min_key_code; key <= keyboard->max_key_code; ++key) {
            if (memcmp(keyboard->names->keys[key].name, "TLDE", XkbKeyNameLength) == 0) {
                result = key;
                break;
            }
        }
    }
    XkbFreeKeyboard(keyboard, XkbAllComponentsMask, True);
    return result;
}

typedef struct {
    unsigned int super;
    unsigned int locks;
} ModifierMasks;

static ModifierMasks modifier_masks(Display *display) {
    ModifierMasks masks = { .locks = LockMask };
    XModifierKeymap *map = XGetModifierMapping(display);
    if (!map) return masks;

    for (int modifier = 0; modifier < 8; ++modifier) {
        unsigned int bit = 1u << modifier;
        for (int slot = 0; slot < map->max_keypermod; ++slot) {
            KeyCode code = map->modifiermap[modifier * map->max_keypermod + slot];
            if (code == 0) continue;

            KeySym symbol = XkbKeycodeToKeysym(display, code, 0, 0);
            if (symbol == XK_Super_L || symbol == XK_Super_R) masks.super |= bit;
            if (symbol == XK_Num_Lock || symbol == XK_Scroll_Lock) masks.locks |= bit;
        }
    }
    XFreeModifiermap(map);
    masks.locks &= ~masks.super;
    return masks;
}

static int register_x11(Display *display, int previous_key) {
    int key = physical_grave_key(display);
    ModifierMasks masks = modifier_masks(display);
    if (!key || !masks.super) {
        fputs("No physical TLDE key or Super modifier in the X11 keymap.\n", stderr);
        return 0;
    }

    XSync(display, False);
    grab_error = 0;
    XErrorHandler previous_handler = XSetErrorHandler(remember_x11_error);
    for (int screen = 0; screen < ScreenCount(display); ++screen) {
        Window root = RootWindow(display, screen);
        if (previous_key) XUngrabKey(display, previous_key, AnyModifier, root);
        // Allow all combinations of Caps, Num and Scroll lock, plus either or both Super keys.
        for (unsigned int modifiers = masks.super; modifiers; modifiers = (modifiers - 1) & masks.super) {
            unsigned int locks = masks.locks;
            do {
                XGrabKey(display, key, modifiers | locks, root, False, GrabModeAsync, GrabModeAsync);
                if (!locks) break;
                locks = (locks - 1) & masks.locks;
            } while (True);
        }
    }
    XSync(display, False);
    XSetErrorHandler(previous_handler);
    if (grab_error) {
        fputs(grab_error == BadAccess
            ? "Super+grave is already registered by another X11 client.\n"
            : "X11 could not register Super+grave.\n", stderr);
        return 0;
    }
    return key;
}

static int x11_loop(SaveCallback save, const void *data) {
    Display *display = XOpenDisplay(NULL);
    if (!display) {
        fputs("Could not open the X11 display.\n", stderr);
        return 1;
    }

    Bool detectable_repeat = False;
    XkbSetDetectableAutoRepeat(display, True, &detectable_repeat);
    int key = register_x11(display, 0);
    if (!key) {
        XCloseDisplay(display);
        return 1;
    }

    fputs("Backtracking is listening for Super+grave on X11.\n", stderr);
    Bool pressed = False;
    for (;;) {
        XEvent event;
        XNextEvent(display, &event);
        if (event.type == MappingNotify) {
            XRefreshKeyboardMapping(&event.xmapping);
            if (event.xmapping.request != MappingPointer) {
                key = register_x11(display, key);
                if (!key) break;
                pressed = False;
            }
            continue;
        }
        if ((event.type != KeyPress && event.type != KeyRelease)
            || event.xkey.keycode != (unsigned int)key || event.xkey.send_event) continue;

        if (event.type == KeyPress) {
            if (!pressed) save(data);
            pressed = True;
        } else {
            if (!detectable_repeat && XEventsQueued(display, QueuedAfterReading)) {
                XEvent next;
                XPeekEvent(display, &next);
                if (next.type == KeyPress && next.xkey.keycode == event.xkey.keycode
                    && next.xkey.time == event.xkey.time) continue;
            }
            pressed = False;
        }
    }
    XCloseDisplay(display);
    return 1;
}

#define PORTAL "org.freedesktop.portal.Desktop"
#define DESKTOP_PATH "/org/freedesktop/portal/desktop"
#define GLOBAL_SHORTCUTS "org.freedesktop.portal.GlobalShortcuts"
#define REQUEST "org.freedesktop.portal.Request"
#define SESSION "org.freedesktop.portal.Session"
#define SHORTCUT_ID "save-snapshot"

typedef struct {
    GMainLoop *loop;
    gchar *handle;
    GVariant *response;
    guint timer;
} PortalRequest;

static void portal_response(GDBusConnection *bus, const gchar *sender, const gchar *path,
                            const gchar *interface, const gchar *signal, GVariant *body,
                            gpointer data) {
    (void)bus;
    (void)sender;
    (void)interface;
    (void)signal;
    PortalRequest *request = data;
    if (!request->response && g_strcmp0(request->handle, path) == 0) {
        request->response = g_variant_ref(body);
        g_main_loop_quit(request->loop);
    }
}

static gboolean portal_timeout(gpointer data) {
    PortalRequest *request = data;
    request->timer = 0;
    g_main_loop_quit(request->loop);
    return G_SOURCE_REMOVE;
}

static GVariant *portal_request(GDBusConnection *bus, const char *method, GVariant *options,
                                GError **error) {
    PortalRequest request = { .loop = g_main_loop_new(NULL, FALSE) };
    // Subscribe first: the backend can send Response immediately after returning the handle.
    guint subscription = g_dbus_connection_signal_subscribe(
        bus, PORTAL, REQUEST, "Response", NULL, NULL, G_DBUS_SIGNAL_FLAGS_NONE,
        portal_response, &request, NULL);
    GVariant *reply = g_dbus_connection_call_sync(
        bus, PORTAL, DESKTOP_PATH, GLOBAL_SHORTCUTS, method, options, G_VARIANT_TYPE("(o)"),
        G_DBUS_CALL_FLAGS_NONE, 5000, NULL, error);
    if (reply) {
        g_variant_get(reply, "(o)", &request.handle);
        g_variant_unref(reply);
        request.timer = g_timeout_add_seconds(120, portal_timeout, &request);
        g_main_loop_run(request.loop);
        if (request.timer) g_source_remove(request.timer);
    }

    g_dbus_connection_signal_unsubscribe(bus, subscription);
    g_main_loop_unref(request.loop);
    g_free(request.handle);
    if (!request.response) {
        if (!*error) g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_TIMED_OUT,
                                        "The shortcut portal did not respond.");
        return NULL;
    }

    guint32 status;
    GVariant *result;
    g_variant_get(request.response, "(u@a{sv})", &status, &result);
    g_variant_unref(request.response);
    if (status) {
        g_variant_unref(result);
        g_set_error_literal(error, G_IO_ERROR, G_IO_ERROR_FAILED,
                            "The desktop declined the shortcut request.");
        return NULL;
    }
    return result;
}

typedef struct {
    const gchar *session;
    GMainLoop *loop;
    SaveCallback save;
    const void *data;
    gboolean pressed;
} PortalLoop;

static void shortcut_signal(GDBusConnection *bus, const gchar *sender, const gchar *path,
                            const gchar *interface, const gchar *signal, GVariant *body,
                            gpointer data) {
    (void)bus;
    (void)sender;
    (void)interface;
    PortalLoop *loop = data;
    if (strcmp(signal, "Closed") == 0) {
        if (strcmp(path, loop->session) == 0) g_main_loop_quit(loop->loop);
        return;
    }

    const gchar *session;
    const gchar *shortcut;
    g_variant_get_child(body, 0, "&o", &session);
    g_variant_get_child(body, 1, "&s", &shortcut);
    if (strcmp(session, loop->session) != 0 || strcmp(shortcut, SHORTCUT_ID) != 0) return;

    if (strcmp(signal, "Activated") == 0) {
        if (!loop->pressed) loop->save(loop->data);
        loop->pressed = TRUE;
    } else if (strcmp(signal, "Deactivated") == 0) {
        loop->pressed = FALSE;
    }
}

static int portal_loop(SaveCallback save, const void *data) {
    GError *error = NULL;
    GDBusConnection *bus = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &error);
    if (!bus) goto failed;

    // Older portal versions do not expose the Registry; their shortcut backend can
    // still accept the session without this optional registration.
    GVariant *registered = g_dbus_connection_call_sync(
        bus, PORTAL, DESKTOP_PATH, "org.freedesktop.host.portal.Registry", "Register",
        g_variant_new_parsed("('tw.kamelyang.backtracking.shortcut', @a{sv} {})"),
        NULL, G_DBUS_CALL_FLAGS_NONE, 5000, NULL, &error);
    if (registered) {
        g_variant_unref(registered);
    } else if (g_error_matches(error, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_INTERFACE)
               || g_error_matches(error, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD)) {
        g_clear_error(&error);
    } else {
        goto failed;
    }

    GVariant *created = portal_request(
        bus, "CreateSession", g_variant_new_parsed("({'handle_token': <'backtracking_create'>,"
           " 'session_handle_token': <'backtracking_session'>},)"), &error);
    if (!created) goto failed;

    // The portal documents a legacy string and also permits an object path.
    gchar *session = NULL;
    GVariant *handle = g_variant_lookup_value(created, "session_handle", NULL);
    if (handle) {
        if (g_variant_is_of_type(handle, G_VARIANT_TYPE_STRING)
            || g_variant_is_of_type(handle, G_VARIANT_TYPE_OBJECT_PATH)) {
            session = g_variant_dup_string(handle, NULL);
        }
        g_variant_unref(handle);
    }
    g_variant_unref(created);
    if (!session || !g_variant_is_object_path(session)) {
        g_free(session);
        g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                            "The shortcut portal did not return a valid session.");
        goto failed;
    }

    PortalLoop loop = { .session = session, .loop = g_main_loop_new(NULL, FALSE),
                        .save = save, .data = data };
    guint activated = g_dbus_connection_signal_subscribe(
        bus, PORTAL, GLOBAL_SHORTCUTS, "Activated", DESKTOP_PATH, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, shortcut_signal, &loop, NULL);
    guint deactivated = g_dbus_connection_signal_subscribe(
        bus, PORTAL, GLOBAL_SHORTCUTS, "Deactivated", DESKTOP_PATH, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, shortcut_signal, &loop, NULL);
    guint closed = g_dbus_connection_signal_subscribe(
        bus, PORTAL, SESSION, "Closed", session, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, shortcut_signal, &loop, NULL);

    GVariant *bound = portal_request(
        bus, "BindShortcuts", g_variant_new_parsed("(%o, [('" SHORTCUT_ID "',"
            " {'description': <'Save Backtracking snapshot'>, 'preferred_trigger': <'LOGO+grave'>})],"
            " '', {'handle_token': <'backtracking_bind'>})", session), &error);
    if (bound) {
        GVariant *shortcuts = g_variant_lookup_value(bound, "shortcuts", G_VARIANT_TYPE("a(sa{sv})"));
        if (shortcuts && g_variant_n_children(shortcuts) > 0) {
            fputs("Backtracking is listening for the shortcut configured by the desktop.\n", stderr);
            g_main_loop_run(loop.loop);
        } else {
            g_set_error_literal(&error, G_IO_ERROR, G_IO_ERROR_FAILED,
                                "No shortcut was bound by the desktop.");
        }
        g_clear_pointer(&shortcuts, g_variant_unref);
        g_variant_unref(bound);
    }

    g_dbus_connection_signal_unsubscribe(bus, closed);
    g_dbus_connection_signal_unsubscribe(bus, deactivated);
    g_dbus_connection_signal_unsubscribe(bus, activated);
    g_main_loop_unref(loop.loop);
    g_free(session);
    if (!error) {
        g_object_unref(bus);
        return 0;
    }

failed:
    fprintf(stderr, "Wayland shortcut portal: %s\n",
            error ? error->message : "could not create the session");
    fputs("Configure a compositor shortcut to run backtracking-shortcut --save if needed.\n", stderr);
    g_clear_error(&error);
    g_clear_object(&bus);
    return 1;
}

int bt_linux_hotkey_run(int wayland, SaveCallback save, const void *data) {
    return wayland ? portal_loop(save, data) : x11_loop(save, data);
}
