/*
 * x11grab_probe — Mode B 关键机制实证探针(借鉴 sxhkd/xremap 的 grab 用法)
 *
 * 验证:XGrabKey 同步抓键后,
 *   - XAllowEvents(dpy, ReplayKeyboard, time) 把按键"原样"还给焦点窗口(应用完全无感);
 *   - XAllowEvents(dpy, AsyncKeyboard, time) 吞掉按键(应用收不到)。
 *
 * 用法:./x11grab_probe <display>  ;然后另开终端向该屏任意聚焦窗口打字。
 * 行为约定:Space → Replay(放行);字母 a → Async(吞掉);Ctrl+C 退出。
 * 输出:每个决策一行 PASS:xx / EAT:xx,便于 e2e 脚本断言。
 */
#include <X11/Xlib.h>
#include <X11/keysym.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
    const char *disp = argc > 1 ? argv[1] : ":99";
    Display *dpy = XOpenDisplay(disp);
    if (!dpy) { fprintf(stderr, "cannot open %s\n", disp); return 1; }
    Window root = DefaultRootWindow(dpy);

    /* 修饰键组合全集:处理 NumLock(M2)/CapsLock(Lock),借鉴 sxhkd grab_mask 集合 */
    unsigned int mods[] = { 0, ShiftMask, LockMask, Mod2Mask,
                            ShiftMask | LockMask, ShiftMask | Mod2Mask,
                            LockMask | Mod2Mask, ShiftMask | LockMask | Mod2Mask };
    int kc_space = XKeysymToKeycode(dpy, XK_space);
    int kc_a     = XKeysymToKeycode(dpy, XK_a);
    for (unsigned long i = 0; i < sizeof(mods)/sizeof(mods[0]); i++) {
        XGrabKey(dpy, kc_space, mods[i], root, False, GrabModeSync, GrabModeAsync);
        XGrabKey(dpy, kc_a,     mods[i], root, False, GrabModeSync, GrabModeAsync);
    }
    XSelectInput(dpy, root, KeyPressMask | KeyReleaseMask);
    XSync(dpy, False);
    printf("probe ready: space=REPLAY a=EAT (keycodes %d/%d)\n", kc_space, kc_a);
    fflush(stdout);

    XEvent ev;
    for (;;) {
        XNextEvent(dpy, &ev);
        if (ev.type != KeyPress && ev.type != KeyRelease) continue;
        int ks = XkbKeycodeToKeysym(dpy, ev.xkey.keycode, 0, 0);
        int replay = (ks == XK_space);
        /* 同步 grab 冻结了键盘:必须先裁定再解冻 */
        XAllowEvents(dpy, replay ? ReplayKeyboard : AsyncKeyboard, ev.xkey.time);
        printf("%s:%s kc=%d %s\n", replay ? "PASS" : "EAT",
               ev.type == KeyPress ? "press" : "release",
               ev.xkey.keycode, XKeysymToString(ks));
        fflush(stdout);
    }
    return 0;
}
