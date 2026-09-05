/*
 * x11grab_probe_echo — Mode B 放行通道设计实证(吞掉 + XTest 回声)
 *
 * 背景:在本机 X server 上实测 XAllowEvents(ReplayKeyboard) 无法把被动抓取的键
 * 送回焦点窗口(见 RESEARCH.md §2),因此放行改用"摘 grab → AsyncKeyboard 吞掉
 * 当前事件 → XTest 重放 → 重挂 grab"。由于回放期间 grab 已摘除,XTest 事件不会
 * 触发自身 grab,机制上杜绝回声死循环;代价是摘/挂之间(微秒级)用户按键会直接
 * 透传,无副作用。
 *
 * 行为约定:Space → 吞掉并 XTest 回声(应用应收到);字母 a → 直接吞掉。
 * 判定:probe 日志每键一行 PASS(echo)/EAT;xev 日志应出现 space、不出现 a。
 */
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/keysym.h>
#include <X11/extensions/XTest.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

static Display *dpy;
static Window root;
static int kc_pass, kc_eat;

static void grab_all(int kc, int grab) /* grab=1 挂 / 0 摘,含 NumLock/CapsLock 组合 */
{
    static const unsigned int mods[] = { 0, ShiftMask, LockMask, Mod2Mask,
        ShiftMask | LockMask, ShiftMask | Mod2Mask,
        LockMask | Mod2Mask, ShiftMask | LockMask | Mod2Mask };
    for (unsigned long i = 0; i < sizeof(mods)/sizeof(mods[0]); i++) {
        if (grab) XGrabKey(dpy, kc, mods[i], root, False, GrabModeSync, GrabModeAsync);
        else      XUngrabKey(dpy, kc, mods[i], root);
    }
}

int main(int argc, char **argv)
{
    dpy = XOpenDisplay(argc > 1 ? argv[1] : ":99");
    if (!dpy) { fprintf(stderr, "cannot open display\n"); return 1; }
    root = DefaultRootWindow(dpy);
    kc_pass = XKeysymToKeycode(dpy, XK_space);
    kc_eat  = XKeysymToKeycode(dpy, XK_a);
    grab_all(kc_pass, 1);
    grab_all(kc_eat, 1);
    XSelectInput(dpy, root, KeyPressMask | KeyReleaseMask);
    XSync(dpy, False);
    printf("echo probe ready: space=PASS(echo) a=EAT\n"); fflush(stdout);

    XEvent ev;
    for (;;) {
        XNextEvent(dpy, &ev);
        if (ev.type != KeyPress && ev.type != KeyRelease) continue;
        KeySym ks = XLookupKeysym(&ev.xkey, 0);
        if (ks == XK_space) {
            /* 放行:摘 grab → 解冻(吞掉本事件)→ XTest 重放 → 重挂 grab */
            grab_all(kc_pass, 0);
            XAllowEvents(dpy, AsyncKeyboard, ev.xkey.time);
            XSync(dpy, False);
            XTestFakeKeyEvent(dpy, kc_pass, ev.type == KeyPress, CurrentTime);
            XSync(dpy, False);
            grab_all(kc_pass, 1);
            XSync(dpy, False);
            printf("PASS:%s kc=%d\n", ev.type == KeyPress ? "press" : "release", ev.xkey.keycode);
        } else {
            XAllowEvents(dpy, AsyncKeyboard, ev.xkey.time);
            printf("EAT:%s kc=%d\n", ev.type == KeyPress ? "press" : "release", ev.xkey.keycode);
        }
        fflush(stdout);
    }
}
