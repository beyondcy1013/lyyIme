/*
 * 全局截屏快捷键(global_hotkey.h 实现,合同 §13)
 *
 * 通道:org.xfce.Xfconf / org/xfce/Xfconf / org.xfce.Xfconf
 * (SetProperty/GetProperty/GetAllProperties/PropertyExists/ResetProperty),
 * channel = xfce4-keyboard-shortcuts。
 *
 * 绑定判定与冲突规则:
 *   - 只有 commands/{custom,default}/ 与 xfwm4/{custom,default}/ 的**直接
 *     子叶**属性算按键绑定;override、startup-notify、嵌套路径一律忽略;
 *   - 有效分支由对应 custom/override 决定(true→custom,false→default);
 *   - 等效组合按归一化 (keyval, 修饰位) 比较(<Control>≡<Primary>、
 *     修饰顺序无关、键名大小写归一),不做字符串比较;
 *   - 有效绑定中同组合且值非空、命令又非 /usr/local/bin/lyyime-shot 者
 *     一律冲突拒绝(先报错再写,绝不覆盖他人绑定);
 *   - /commands/custom/ 下字符串值恰为安装路径者为自有绑定:目标键保留,
 *     其余(含等效别名写法)在登记成功后撤下,保证全局只剩一把截屏键。
 */
#include "global_hotkey.h"

#include <gtk/gtk.h>
#include <string.h>
#include <xcb/xproto.h>

#include "keysym_map.h"

#define LYY_XFCONF_SERVICE "org.xfce.Xfconf"
#define LYY_XFCONF_PATH "/org/xfce/Xfconf"
#define LYY_XFCONF_IFACE "org.xfce.Xfconf"
#define LYY_XFCONF_CHANNEL "xfce4-keyboard-shortcuts"
#define LYY_SHOT_COMMAND "/usr/local/bin/lyyime-shot"
#define LYY_GH_TIMEOUT_MS 3000

static GQuark lyy_gh_quark(void)
{
    return g_quark_from_static_string("lyy-global-hotkey");
}
#define LYY_GH_ERROR lyy_gh_quark()

/* ---- 事务对象 ---- */

typedef struct {
    gchar *prop;      /* 绝对属性路径 */
    gboolean existed; /* 变更前已存在(回滚=Set 旧值;否则=Reset) */
    GVariant *old;    /* existed=TRUE 时变更前的值 */
} LyyGhOp;

struct LyyGlobalHotkeyChange {
    GDBusConnection *conn;    /* 会话总线连接(g_bus_get_sync 引用) */
    GHashTable *snapshot;     /* prop → GVariant*(变更前全量快照) */
    GPtrArray *ops;           /* LyyGhOp*:实际发生的变更(逆序回滚) */
};

static void gh_op_free(gpointer p)
{
    LyyGhOp *op = p;
    g_free(op->prop);
    if (op->old)
        g_variant_unref(op->old);
    g_free(op);
}

static void gh_track(LyyGlobalHotkeyChange *ch, const gchar *prop,
                     gboolean existed, GVariant *old)
{
    LyyGhOp *op = g_new0(LyyGhOp, 1);
    op->prop = g_strdup(prop);
    op->existed = existed;
    if (old)
        op->old = g_variant_ref(old); /* 快照/重读值均已归一为非浮点 */
    g_ptr_array_add(ch->ops, op);
}

/* GLib 取出的 GVariant 可能带浮点引用:归一为我方持有的 1 个硬引用
 * (浮点→ref_sink;已有硬引用→原样),供快照存储与回滚复用 */
static GVariant *gh_own(GVariant *v)
{
    return g_variant_is_floating(v) ? g_variant_ref_sink(v) : v;
}

/* ---- DBus 基础件 ---- */

static GVariant *gh_call(GDBusConnection *conn, const gchar *method,
                         GVariant *params, const GVariantType *reply_type,
                         GError **error)
{
    return g_dbus_connection_call_sync(
        conn, LYY_XFCONF_SERVICE, LYY_XFCONF_PATH, LYY_XFCONF_IFACE, method,
        params, reply_type, G_DBUS_CALL_FLAGS_NONE, LYY_GH_TIMEOUT_MS, NULL,
        error);
}

static gboolean gh_set(GDBusConnection *conn, const gchar *prop,
                       GVariant *value, GError **error)
{
    GError *err = NULL;
    GVariant *r = gh_call(
        conn, "SetProperty",
        g_variant_new("(ssv)", LYY_XFCONF_CHANNEL, prop, value),
        G_VARIANT_TYPE("()"), &err);
    if (r) {
        g_variant_unref(r);
        return TRUE;
    }
    g_set_error(error, LYY_GH_ERROR, 0, "写入快捷键属性 %s 失败:%s", prop,
                err ? err->message : "未知错误");
    g_clear_error(&err);
    return FALSE;
}

static gboolean gh_reset(GDBusConnection *conn, const gchar *prop,
                         GError **error)
{
    GError *err = NULL;
    GVariant *r = gh_call(conn, "ResetProperty",
                          g_variant_new("(ssb)", LYY_XFCONF_CHANNEL, prop,
                                        FALSE),
                          G_VARIANT_TYPE("()"), &err);
    if (r) {
        g_variant_unref(r);
        return TRUE;
    }
    g_set_error(error, LYY_GH_ERROR, 0, "移除快捷键属性 %s 失败:%s", prop,
                err ? err->message : "未知错误");
    g_clear_error(&err);
    return FALSE;
}

static gboolean gh_exists(GDBusConnection *conn, const gchar *prop,
                          gboolean *exists, GError **error)
{
    GError *err = NULL;
    GVariant *r = gh_call(conn, "PropertyExists",
                          g_variant_new("(ss)", LYY_XFCONF_CHANNEL, prop),
                          G_VARIANT_TYPE("(b)"), &err);
    if (!r) {
        g_set_error(error, LYY_GH_ERROR, 0, "查询快捷键属性 %s 失败:%s",
                    prop, err ? err->message : "未知错误");
        g_clear_error(&err);
        return FALSE;
    }
    g_variant_get(r, "(b)", exists);
    g_variant_unref(r);
    return TRUE;
}

static GVariant *gh_get(GDBusConnection *conn, const gchar *prop,
                        GError **error)
{
    GError *err = NULL;
    GVariant *r = gh_call(conn, "GetProperty",
                          g_variant_new("(ss)", LYY_XFCONF_CHANNEL, prop),
                          G_VARIANT_TYPE("(v)"), &err);
    if (!r) {
        g_set_error(error, LYY_GH_ERROR, 0, "读取快捷键属性 %s 失败:%s",
                    prop, err ? err->message : "未知错误");
        g_clear_error(&err);
        return NULL;
    }
    /* 回复是 (v):子值仍是 variant 包装,须再解一层才是属性真值 */
    GVariant *boxed = g_variant_get_child_value(r, 0);
    GVariant *inner = g_variant_get_variant(boxed);
    g_variant_unref(boxed);
    g_variant_unref(r);
    return gh_own(inner);
}

/* ---- 快照 / 属性判读 ---- */

static GHashTable *gh_snapshot(GDBusConnection *conn, GError **error)
{
    GError *err = NULL;
    GVariant *reply =
        gh_call(conn, "GetAllProperties",
                g_variant_new("(ss)", LYY_XFCONF_CHANNEL, ""),
                G_VARIANT_TYPE("(a{sv})"), &err);
    if (!reply) {
        g_set_error(error, LYY_GH_ERROR, 0,
                    "读取 XFCE 快捷键配置失败:%s",
                    err ? err->message : "未知错误");
        g_clear_error(&err);
        return NULL;
    }
    GVariant *dict = g_variant_get_child_value(reply, 0);
    g_variant_unref(reply);
    GHashTable *snap =
        g_hash_table_new_full(g_str_hash, g_str_equal, g_free,
                              (GDestroyNotify)g_variant_unref);
    GVariantIter it;
    gchar *prop = NULL;
    GVariant *val = NULL;
    g_variant_iter_init(&it, dict);
    while (g_variant_iter_next(&it, "{sv}", &prop, &val))
        g_hash_table_insert(snap, prop, gh_own(val));
    g_variant_unref(dict);
    return snap;
}

static gboolean gh_snap_bool(GHashTable *snap, const gchar *prop)
{
    GVariant *v = g_hash_table_lookup(snap, prop);
    return v && g_variant_is_of_type(v, G_VARIANT_TYPE_BOOLEAN) &&
           g_variant_get_boolean(v);
}

typedef enum {
    GH_BRANCH_NONE = 0,
    GH_CMD_DEFAULT,
    GH_CMD_CUSTOM,
    GH_WM_DEFAULT,
    GH_WM_CUSTOM
} GhBranch;

/* 属性路径 → 分支 + 加速键名尾段;只有直接子叶才算绑定
 * (override 开关 / startup-notify / 嵌套路径均剔除) */
static gboolean gh_classify(const gchar *prop, GhBranch *branch,
                            const gchar **accel)
{
    static const struct {
        const gchar *prefix;
        GhBranch branch;
    } T[] = {
        { "/commands/custom/", GH_CMD_CUSTOM },
        { "/commands/default/", GH_CMD_DEFAULT },
        { "/xfwm4/custom/", GH_WM_CUSTOM },
        { "/xfwm4/default/", GH_WM_DEFAULT },
    };
    for (size_t i = 0; i < G_N_ELEMENTS(T); i++) {
        if (!g_str_has_prefix(prop, T[i].prefix))
            continue;
        const gchar *rest = prop + strlen(T[i].prefix);
        if (!*rest || strchr(rest, '/'))
            return FALSE;
        if (!strcmp(rest, "override"))
            return FALSE;
        *branch = T[i].branch;
        *accel = rest;
        return TRUE;
    }
    return FALSE;
}

/* GTK accelerator 写法 → 归一化 (keyval, 修饰位);解析失败=FALSE
 * (无法判等,不参与冲突与归属判定) */
static gboolean gh_parse_accel(const gchar *name, guint *key_out,
                               GdkModifierType *mods_out)
{
    guint key = 0;
    GdkModifierType mods = 0;
    gtk_accelerator_parse(name, &key, &mods);
    if (key == 0)
        return FALSE;
    /* <Mod4> 与 <Super> 同位:归一到 GDK_SUPER_MASK 再比等效;
     * 其它未识别掩码原样保留(不丢弃,避免虚构等效) */
    if (mods & GDK_MOD4_MASK)
        mods = (mods & ~GDK_MOD4_MASK) | GDK_SUPER_MASK;
    *key_out = gdk_keyval_to_lower(key);
    *mods_out = mods;
    return TRUE;
}

/* XDG_CURRENT_DESKTOP 显式为非 XFCE 时拒绝(不自动外推其它桌面) */
static gboolean gh_desktop_supported(GError **error)
{
    const gchar *desk = g_getenv("XDG_CURRENT_DESKTOP");
    if (!desk || !*desk)
        return TRUE;
    gchar **items = g_strsplit(desk, ":", -1);
    gboolean xfce = FALSE;
    for (gchar **p = items; *p; p++)
        if (g_ascii_strcasecmp(*p, "XFCE") == 0)
            xfce = TRUE;
    g_strfreev(items);
    if (xfce)
        return TRUE;
    g_set_error(error, LYY_GH_ERROR, 0,
                "当前桌面环境为「%s」,全局截屏快捷键仅支持 XFCE;"
                "请在所用桌面的快捷键设置中自行绑定 %s。",
                desk, LYY_SHOT_COMMAND);
    return FALSE;
}

static GDBusConnection *gh_bus_connect(GError **error)
{
    GError *err = NULL;
    GDBusConnection *conn = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &err);
    if (!conn)
        g_set_error(error, LYY_GH_ERROR, 0,
                    "无法连接桌面会话总线(D-Bus):%s",
                    err ? err->message : "未知错误");
    g_clear_error(&err);
    return conn;
}

/* 逆序回放已发生的变更:存在过的恢复旧值(Set),新建的直接删(Reset) */
static gboolean gh_rollback_ops(LyyGlobalHotkeyChange *ch, GError **error)
{
    gboolean ok = TRUE;
    for (gint i = (gint)ch->ops->len - 1; i >= 0; i--) {
        LyyGhOp *op = g_ptr_array_index(ch->ops, (guint)i);
        GError *step = NULL;
        gboolean done = op->existed
                            ? gh_set(ch->conn, op->prop, op->old, &step)
                            : gh_reset(ch->conn, op->prop, &step);
        if (!done && ok) {
            g_set_error(error, LYY_GH_ERROR, 0, "回滚 %s 失败:%s", op->prop,
                        step ? step->message : "未知错误");
            ok = FALSE;
        }
        g_clear_error(&step);
    }
    return ok;
}

/* 事务收尾:把回滚结果并入原错误,释放事务 */
static void gh_fail(LyyGlobalHotkeyChange *ch, GError **error)
{
    GError *rb = NULL;
    if (!gh_rollback_ops(ch, &rb) && error && *error) {
        gchar *msg = g_strdup_printf("%s(回滚亦失败:%s)",
                                     (*error)->message,
                                     rb && rb->message ? rb->message
                                                       : "未知错误");
        g_clear_error(error);
        g_set_error(error, LYY_GH_ERROR, 0, "%s", msg);
        g_free(msg);
    }
    g_clear_error(&rb);
    lyy_global_shot_commit(ch);
}

gchar *lyy_global_shot_accelerator(const char *spec, GError **error)
{
    uint32_t mods = 0, sym = 0;
    if (!lyy_hotkey_parse(spec, &mods, &sym)) {
        g_set_error(error, LYY_GH_ERROR, 0,
                    "截屏快捷键「%s」写法不合法:需至少一个修饰"
                    "(ctrl/alt/super/shift)+键名,如 ctrl+alt+a。",
                    spec ? spec : "");
        return NULL;
    }
    GdkModifierType gmods = 0;
    if (mods & XCB_MOD_MASK_SHIFT)
        gmods |= GDK_SHIFT_MASK;
    if (mods & XCB_MOD_MASK_CONTROL)
        gmods |= GDK_CONTROL_MASK;
    if (mods & XCB_MOD_MASK_1)
        gmods |= GDK_MOD1_MASK;
    if (mods & XCB_MOD_MASK_4)
        gmods |= GDK_SUPER_MASK;
    return gtk_accelerator_name((guint)sym, gmods);
}

LyyGlobalHotkeyChange *lyy_global_shot_apply(const char *spec,
                                             GError **error)
{
    g_return_val_if_fail(error == NULL || *error == NULL, NULL);

    gchar *accel = lyy_global_shot_accelerator(spec, error);
    if (!accel)
        return NULL;
    /* 绑定命令是固定安装路径:登记前先确认二进制可执行 */
    if (!g_file_test(LYY_SHOT_COMMAND, G_FILE_TEST_IS_EXECUTABLE)) {
        g_set_error(error, LYY_GH_ERROR, 0,
                    "截屏程序 %s 不存在或不可执行,未登记全局快捷键"
                    "(请先完成 lyyime-shot 安装)。",
                    LYY_SHOT_COMMAND);
        g_free(accel);
        return NULL;
    }
    if (!gh_desktop_supported(error)) {
        g_free(accel);
        return NULL;
    }

    LyyGlobalHotkeyChange *ch = g_new0(LyyGlobalHotkeyChange, 1);
    ch->ops = g_ptr_array_new_with_free_func(gh_op_free);
    gchar *target = g_strdup_printf("/commands/custom/%s", accel);
    GPtrArray *owned = g_ptr_array_new(); /* 自有绑定 prop(借用快照键) */
    guint tkey = 0;
    GdkModifierType tmods = 0;
    gboolean ok = FALSE;

    ch->conn = gh_bus_connect(error);
    if (!ch->conn)
        goto out;
    ch->snapshot = gh_snapshot(ch->conn, error);
    if (!ch->snapshot)
        goto out;
    if (!gh_parse_accel(accel, &tkey, &tmods)) {
        g_set_error(error, LYY_GH_ERROR, 0,
                    "内部错误:生成的 accelerator「%s」无法回解析。", accel);
        goto out;
    }

    /* 自定义快捷键组未启用时绝不代为打开(会连带激活用户全部自定义
     * 绑定),报人话错误让上层弹窗 */
    if (!gh_snap_bool(ch->snapshot, "/commands/custom/override")) {
        g_set_error(error, LYY_GH_ERROR, 0,
                    "XFCE 自定义应用程序快捷键未启用"
                    "(commands/custom/override):请在「设置→键盘→"
                    "应用程序快捷键」确认启用后再保存。");
        goto out;
    }
    gboolean wm_custom =
        gh_snap_bool(ch->snapshot, "/xfwm4/custom/override");

    /* 冲突扫描(任何 DBus 写入之前):有效分支内等效组合 + 非空值
     * + 非我方命令 ⇒ 拒绝并点名占用属性 */
    {
        GHashTableIter it;
        gchar *prop = NULL;
        GVariant *val = NULL;
        g_hash_table_iter_init(&it, ch->snapshot);
        while (g_hash_table_iter_next(&it, (gpointer)&prop,
                                      (gpointer)&val)) {
            GhBranch br = GH_BRANCH_NONE;
            const gchar *name = NULL;
            if (!gh_classify(prop, &br, &name))
                continue;
            guint key = 0;
            GdkModifierType mods = 0;
            if (!gh_parse_accel(name, &key, &mods))
                continue;
            gboolean is_custom =
                (br == GH_CMD_CUSTOM || br == GH_WM_CUSTOM);
            gboolean eff_custom =
                (br == GH_CMD_CUSTOM || br == GH_CMD_DEFAULT)
                    ? TRUE /* commands/override 上面已确认 */
                    : wm_custom;
            gboolean active = (is_custom == eff_custom);
            const gchar *vstr =
                g_variant_is_of_type(val, G_VARIANT_TYPE_STRING)
                    ? g_variant_get_string(val, NULL)
                    : NULL;
            /* 自有 = commands/custom 直接子叶且命令恰为安装路径;
             * WM 绑定即便值相同也不算自有,同组合照旧判冲突 */
            gboolean is_owned = (br == GH_CMD_CUSTOM && vstr &&
                                 !strcmp(vstr, LYY_SHOT_COMMAND));
            if (is_owned)
                g_ptr_array_add(owned, prop); /* 自有绑定(收养/待清理) */
            if (active && key == tkey && mods == tmods && vstr && *vstr &&
                !is_owned) {
                g_set_error(error, LYY_GH_ERROR, 0,
                            "截屏快捷键「%s」与系统现有绑定冲突:"
                            "%s = %s。\n请更换组合,或先在 XFCE 键盘"
                            "设置中移除该绑定(不会自动覆盖)。",
                            spec, prop, vstr);
                goto out;
            }
        }
    }

    /* 变更一:登记目标属性。写前重读现值(快照到落笔之间有窗口期,
     * 外部并发改动不保证原子互斥,但至少不会覆盖刚被他人占用的键):
     * 现值非空且非我方命令 → 拒绝;现值已是同值 → 跳过(幂等)。 */
    {
        gboolean t_exists = FALSE;
        if (!gh_exists(ch->conn, target, &t_exists, error))
            goto out;
        GVariant *tcur = NULL;
        if (t_exists) {
            tcur = gh_get(ch->conn, target, error);
            if (!tcur)
                goto out;
        }
        const gchar *ts =
            tcur && g_variant_is_of_type(tcur, G_VARIANT_TYPE_STRING)
                ? g_variant_get_string(tcur, NULL)
                : NULL;
        if (t_exists && (!ts || (*ts && strcmp(ts, LYY_SHOT_COMMAND)))) {
            g_set_error(error, LYY_GH_ERROR, 0,
                        "截屏快捷键目标属性 %s 当前被占用"
                        "(非空且非截屏命令),未改写。", target);
            g_clear_pointer(&tcur, g_variant_unref);
            goto out;
        }
        if (!ts || strcmp(ts, LYY_SHOT_COMMAND)) {
            if (!gh_set(ch->conn, target,
                        g_variant_new_string(LYY_SHOT_COMMAND), error)) {
                g_clear_pointer(&tcur, g_variant_unref);
                goto out;
            }
            gh_track(ch, target, t_exists, tcur); /* 回滚恢复现值 */
        }
        g_clear_pointer(&tcur, g_variant_unref);
    }

    /* 变更二:撤下其余自有绑定(旧键 / 等效别名),保证全局只剩目标键;
     * 移除前重读现值,仍是我方命令才删(避免误删并发外部改动) */
    for (guint i = 0; i < owned->len; i++) {
        const gchar *op_prop = g_ptr_array_index(owned, i);
        if (!strcmp(op_prop, target))
            continue;
        gboolean exists = FALSE;
        if (!gh_exists(ch->conn, op_prop, &exists, error))
            goto out;
        if (!exists)
            continue;
        GVariant *now = gh_get(ch->conn, op_prop, error);
        if (!now)
            goto out;
        const gchar *nstr =
            g_variant_is_of_type(now, G_VARIANT_TYPE_STRING)
                ? g_variant_get_string(now, NULL)
                : NULL;
        if (nstr && !strcmp(nstr, LYY_SHOT_COMMAND)) {
            if (!gh_reset(ch->conn, op_prop, error)) {
                g_variant_unref(now);
                goto out;
            }
            gh_track(ch, op_prop, TRUE, now);
        }
        g_variant_unref(now);
    }
    ok = TRUE;

out:
    g_ptr_array_free(owned, TRUE);
    g_free(target);
    g_free(accel);
    if (!ok) {
        gh_fail(ch, error);
        return NULL;
    }
    return ch;
}

gboolean lyy_global_shot_rollback(LyyGlobalHotkeyChange *change,
                                  GError **error)
{
    g_return_val_if_fail(change != NULL, FALSE);
    return gh_rollback_ops(change, error);
}

void lyy_global_shot_commit(LyyGlobalHotkeyChange *change)
{
    if (!change)
        return;
    if (change->conn)
        g_object_unref(change->conn);
    if (change->snapshot)
        g_hash_table_unref(change->snapshot);
    if (change->ops)
        g_ptr_array_unref(change->ops);
    g_free(change);
}

gboolean lyy_global_shot_remove(GError **error)
{
    g_return_val_if_fail(error == NULL || *error == NULL, FALSE);
    if (!gh_desktop_supported(error))
        return FALSE;

    LyyGlobalHotkeyChange *ch = g_new0(LyyGlobalHotkeyChange, 1);
    ch->ops = g_ptr_array_new_with_free_func(gh_op_free);
    gboolean ok = FALSE;

    ch->conn = gh_bus_connect(error);
    if (!ch->conn)
        goto out;
    ch->snapshot = gh_snapshot(ch->conn, error);
    if (!ch->snapshot)
        goto out;

    /* 只删 /commands/custom/ 下命令恰为安装路径的直接子叶(自有绑定);
     * 快照只是候选——删除前逐条重读现值,仍是我方命令才动 */
    {
        GHashTableIter it;
        gchar *prop = NULL;
        GVariant *val = NULL;
        g_hash_table_iter_init(&it, ch->snapshot);
        while (g_hash_table_iter_next(&it, (gpointer)&prop,
                                      (gpointer)&val)) {
            GhBranch br = GH_BRANCH_NONE;
            const gchar *name = NULL;
            if (!gh_classify(prop, &br, &name) || br != GH_CMD_CUSTOM)
                continue;
            const gchar *vstr =
                g_variant_is_of_type(val, G_VARIANT_TYPE_STRING)
                    ? g_variant_get_string(val, NULL)
                    : NULL;
            if (!vstr || strcmp(vstr, LYY_SHOT_COMMAND))
                continue;
            gboolean exists = FALSE;
            if (!gh_exists(ch->conn, prop, &exists, error))
                goto out;
            if (!exists)
                continue;
            GVariant *now = gh_get(ch->conn, prop, error);
            if (!now)
                goto out;
            const gchar *nstr =
                g_variant_is_of_type(now, G_VARIANT_TYPE_STRING)
                    ? g_variant_get_string(now, NULL)
                    : NULL;
            if (nstr && !strcmp(nstr, LYY_SHOT_COMMAND)) {
                if (!gh_reset(ch->conn, prop, error)) {
                    g_variant_unref(now);
                    goto out;
                }
                gh_track(ch, prop, TRUE, now);
            }
            g_variant_unref(now);
        }
    }
    ok = TRUE;

out:
    if (!ok) {
        gh_fail(ch, error);
        return FALSE;
    }
    lyy_global_shot_commit(ch);
    return TRUE;
}
