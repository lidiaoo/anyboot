package com.anyboot.remote;

import android.app.Activity;
import android.content.Context;
import android.content.res.Configuration;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.util.TypedValue;
import android.view.View;
import android.view.Window;

/**
 * 全局 UI 主题（对齐 docs/client-ui/mobile-redesign-all.html 定稿）：
 * OLED 深色为默认，支持亮色；所有界面取色都走这里，不再散落硬编码。
 *
 * <p>主题模式三挡：auto（跟系统深色模式，默认）/ dark / light，切换后 recreate() 生效。
 *
 * <p>自适应：{@link #dp(Context, int)} 在 dp 基础上乘以屏幕宽度因子 ——
 * 360dp 基准 = 1.0，小屏（320dp）≈0.89 收缩，平板/大屏最多放大到 1.25，
 * 配合已有的 ScrollView / FlowLayout / weight 布局覆盖不同分辨率。
 */
public final class Ui {

    private Ui() { }

    public static final String MODE_AUTO = "auto";
    public static final String MODE_DARK = "dark";
    public static final String MODE_LIGHT = "light";

    // ------------------------------------------------------------------
    // 主题模式
    // ------------------------------------------------------------------

    public static String mode(Context context) {
        return ServerStore.prefs(context).getString("ui_theme", MODE_AUTO);
    }

    public static void setMode(Context context, String mode) {
        ServerStore.prefs(context).edit().putString("ui_theme", mode).apply();
    }

    /** 三挡循环：auto → dark → light → auto */
    public static String cycle(Context context) {
        String current = mode(context);
        String next = MODE_AUTO.equals(current) ? MODE_DARK
                : MODE_DARK.equals(current) ? MODE_LIGHT : MODE_AUTO;
        setMode(context, next);
        return next;
    }

    public static String modeLabel(Context context) {
        String current = mode(context);
        if (MODE_DARK.equals(current)) {
            return "暗色";
        }
        if (MODE_LIGHT.equals(current)) {
            return "亮色";
        }
        return "跟随系统";
    }

    public static boolean dark(Context context) {
        String current = mode(context);
        if (MODE_DARK.equals(current)) {
            return true;
        }
        if (MODE_LIGHT.equals(current)) {
            return false;
        }
        int night = context.getResources().getConfiguration().uiMode
                & Configuration.UI_MODE_NIGHT_MASK;
        return night == Configuration.UI_MODE_NIGHT_YES;
    }

    // ------------------------------------------------------------------
    // 调色板（深 / 亮两套，语义同设计稿 token）
    // ------------------------------------------------------------------

    /** 页面底色 */
    public static int base(Context c) {
        return dark(c) ? 0xFF020617 : 0xFFF4F6FA;
    }

    /** 卡片 / 顶栏 / 导航栏底色 */
    public static int card(Context c) {
        return dark(c) ? 0xFF0E1223 : 0xFFFFFFFF;
    }

    /** 输入框 / chip 等再抬一层的底色 */
    public static int elevate(Context c) {
        return dark(c) ? 0xFF141A2E : 0xFFF1F4F9;
    }

    /** 细分隔线 */
    public static int line(Context c) {
        return dark(c) ? 0xFF242B41 : 0xFFE2E8F0;
    }

    /** 描边 / 输入框边框 */
    public static int lineStrong(Context c) {
        return dark(c) ? 0xFF334155 : 0xFFCBD5E1;
    }

    /** 主文字 */
    public static int fg(Context c) {
        return dark(c) ? 0xFFF8FAFC : 0xFF0F172A;
    }

    /** 次要文字 */
    public static int fgDim(Context c) {
        return dark(c) ? 0xFF94A3B8 : 0xFF5A6476;
    }

    /** 弱化文字 / 占位 */
    public static int fgFaint(Context c) {
        return dark(c) ? 0xFF64748B : 0xFF8A94A6;
    }

    /** 主色（终端绿） */
    public static int ok(Context c) {
        return dark(c) ? 0xFF22C55E : 0xFF16A34A;
    }

    /** 主色底上的文字 */
    public static int onAccent(Context c) {
        return dark(c) ? 0xFF052E16 : 0xFFFFFFFF;
    }

    public static int accent(Context c) {
        return ok(c);
    }

    public static int warn(Context c) {
        return dark(c) ? 0xFFF59E0B : 0xFFB45309;
    }

    public static int bad(Context c) {
        return dark(c) ? 0xFFEF4444 : 0xFFDC2626;
    }

    // ------------------------------------------------------------------
    // 自适应尺寸
    // ------------------------------------------------------------------

    /**
     * 宽度自适应因子：以 360dp 宽为基准 1.0，向下最低 0.85（小屏不挤爆），
     * 向上最高 1.25（平板上留白同步放大，不显得空）。
     */
    public static float scale(Context c) {
        android.util.DisplayMetrics metrics = c.getResources().getDisplayMetrics();
        float widthDp = metrics.widthPixels / metrics.density;
        return Math.max(0.85f, Math.min(1.25f, widthDp / 360f));
    }

    /** 所有界面尺寸统一从这里出 —— 带宽度自适应缩放 */
    public static int dp(Context c, int value) {
        float scaled = value * scale(c);
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, scaled,
                c.getResources().getDisplayMetrics()));
    }

    public static GradientDrawable rounded(Context c, int fill, int stroke, int radiusDp) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setShape(GradientDrawable.RECTANGLE);
        drawable.setCornerRadius(dp(c, radiusDp));
        drawable.setColor(fill);
        if (stroke != 0) {
            drawable.setStroke(dp(c, 1), stroke);
        }
        return drawable;
    }

    // ------------------------------------------------------------------
    // 系统栏与对话框
    // ------------------------------------------------------------------

    /** 状态栏 / 导航栏跟随主题底色，图标颜色跟着翻面 */
    public static void applySystemBars(Activity activity) {
        Window window = activity.getWindow();
        int color = base(activity);
        window.setStatusBarColor(color);
        window.setNavigationBarColor(color);
        View decor = window.getDecorView();
        int lightFlags = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR;
        if (Build.VERSION.SDK_INT >= 26) {
            lightFlags |= View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
        }
        int flags = decor.getSystemUiVisibility();
        if (dark(activity)) {
            flags &= ~lightFlags;
        } else {
            flags |= lightFlags;
        }
        decor.setSystemUiVisibility(flags);
    }

    /** Activity 主题：Material 无 ActionBar，弹窗 / 控件 / 下拉随主题变色 */
    public static void applyActivityTheme(Activity activity) {
        activity.setTheme(dark(activity)
                ? android.R.style.Theme_Material_NoActionBar
                : android.R.style.Theme_Material_Light_NoActionBar);
    }

    /** AlertDialog 主题（配 AlertDialog.Builder(ctx, Ui.dialogTheme(ctx))） */
    public static int dialogTheme(Context c) {
        return dark(c)
                ? android.R.style.Theme_Material_Dialog_Alert
                : android.R.style.Theme_Material_Light_Dialog_Alert;
    }
}
