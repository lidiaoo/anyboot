package com.anyboot.remote;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Bundle;
import android.text.InputType;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.Menu;
import android.view.MenuItem;
import android.view.View;
import android.view.ViewGroup;
import android.webkit.CookieManager;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.ArrayAdapter;
import android.widget.AutoCompleteTextView;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.HorizontalScrollView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;
import android.widget.Toast;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * AnyBoot 远程客户端外壳（多服务端版）。
 *
 * <p>它本身不实现业务：每个服务端标签就是一个 {@link WebView}，加载那台电脑上
 * AnyBoot 内置 HTTP 服务托管的前端页面，页面里的 /api/ui-state、/api/ui-command
 * 都是**同源**请求 —— 所以没有 CORS，也不用在这里重复一套 UI：电脑端前端改了，
 * 手机端刷新即最新。
 *
 * <p>手机端额外做四件浏览器不顺手的事：
 * <ol>
 *   <li>同时连多个服务端：每个标签一个独立 WebView，「启动时连接全部」可让它们一起在线；</li>
 *   <li>顶部标签快速切换，标签名是「组名 · 服务端名」，并带在线状态点；</li>
 *   <li>服务端可以分组 —— 一台电脑可能有多个网卡 / 多次 DHCP 拿到不同 IP，
 *       把同一台电脑的几条地址放进同一个分组，标签配色相同，一眼分得清；</li>
 *   <li>断线时给该服务端一个重试面板。</li>
 * </ol>
 */
public class MainActivity extends Activity {

    // 标签上的在线状态
    private static final int STATUS_UNKNOWN = 0;
    private static final int STATUS_LOADING = 1;
    private static final int STATUS_OK = 2;
    private static final int STATUS_FAIL = 3;

    private static final int ACCENT = 0xFF2F6DF6;
    private static final int CHIP_BG = 0xFFF1F3F7;
    private static final int CHIP_BORDER = 0xFFD7DCE4;
    private static final int CHIP_TEXT = 0xFF333A45;
    private static final int COLOR_UNKNOWN = 0xFF9AA0A6;
    private static final int COLOR_LOADING = 0xFFF0A63B;
    private static final int COLOR_OK = 0xFF2E9E5B;
    private static final int COLOR_FAIL = 0xFFD93025;

    /** 一个服务端的运行时状态（WebView 懒创建：没点过的标签不占内存） */
    private static final class Tab {
        ServerProfile profile;
        WebView view;
        int status = STATUS_UNKNOWN;
        boolean mainFailed = false;
        /** 是否已经发起过加载 —— 切回标签时靠它判断「要不要重新拉页面」 */
        boolean started = false;
    }

    private List<ServerProfile> servers = new ArrayList<>();
    private final Map<String, Tab> tabs = new LinkedHashMap<>();
    private final Map<String, LinearLayout> chips = new LinkedHashMap<>();
    private final Map<String, View> dots = new LinkedHashMap<>();

    private String activeId = "";
    private boolean connectAll = false;

    private LinearLayout tabStrip;
    private HorizontalScrollView tabScroll;
    private FrameLayout container;
    private ProgressBar progress;
    private LinearLayout errorPanel;
    private TextView errorText;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        servers = ServerStore.load(this);
        connectAll = ServerStore.connectAll(this);
        activeId = ServerStore.activeId(this);
        buildUi();
        renderTabs();

        if (servers.isEmpty()) {
            showEditDialog(null, true);
            return;
        }
        if (ServerStore.find(servers, activeId) == null) {
            activeId = servers.get(0).id;
            ServerStore.setActiveId(this, activeId);
        }
        activate(activeId);
        if (connectAll) {
            connectAllServers();
        }
    }

    // ------------------------------------------------------------------
    // 界面骨架
    // ------------------------------------------------------------------

    private void buildUi() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(Color.WHITE);

        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setBackgroundColor(0xFFF6F7F9);
        bar.setPadding(dp(8), dp(6), dp(6), dp(6));

        tabScroll = new HorizontalScrollView(this);
        tabScroll.setHorizontalScrollBarEnabled(false);
        tabStrip = new LinearLayout(this);
        tabStrip.setOrientation(LinearLayout.HORIZONTAL);
        tabStrip.setGravity(Gravity.CENTER_VERTICAL);
        tabScroll.addView(tabStrip, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT));
        LinearLayout.LayoutParams scrollParams = new LinearLayout.LayoutParams(
                0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        bar.addView(tabScroll, scrollParams);

        bar.addView(barButton("＋", "添加服务端", v -> showEditDialog(null, false)));
        bar.addView(barButton("☰", "服务端列表", v -> showServerList()));
        root.addView(bar, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT));

        container = new FrameLayout(this);
        root.addView(container, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));

        progress = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        progress.setIndeterminate(true);
        progress.setVisibility(View.GONE);
        FrameLayout.LayoutParams progressParams = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, dp(3));
        progressParams.gravity = Gravity.TOP;
        container.addView(progress, progressParams);

        errorPanel = new LinearLayout(this);
        errorPanel.setOrientation(LinearLayout.VERTICAL);
        errorPanel.setGravity(Gravity.CENTER);
        errorPanel.setBackgroundColor(Color.WHITE);
        errorPanel.setPadding(dp(24), dp(24), dp(24), dp(24));
        errorPanel.setVisibility(View.GONE);

        TextView errorTitle = new TextView(this);
        errorTitle.setText("连不上服务端");
        errorTitle.setTextSize(TypedValue.COMPLEX_UNIT_SP, 20);
        errorTitle.setGravity(Gravity.CENTER);
        errorPanel.addView(errorTitle);

        errorText = new TextView(this);
        errorText.setTextSize(TypedValue.COMPLEX_UNIT_SP, 14);
        errorText.setGravity(Gravity.CENTER);
        errorText.setPadding(0, dp(12), 0, dp(20));
        errorPanel.addView(errorText);

        Button retry = new Button(this);
        retry.setText("重试");
        retry.setOnClickListener(v -> reloadActive());
        errorPanel.addView(retry);

        Button edit = new Button(this);
        edit.setText("修改这个服务端");
        edit.setOnClickListener(v -> showEditDialog(ServerStore.find(servers, activeId), false));
        errorPanel.addView(edit);

        container.addView(errorPanel, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        setContentView(root);
    }

    private TextView barButton(String text, String description, View.OnClickListener listener) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 20);
        view.setGravity(Gravity.CENTER);
        view.setTextColor(0xFF5A6472);
        view.setContentDescription(description);
        view.setPadding(dp(12), dp(4), dp(12), dp(4));
        view.setOnClickListener(listener);
        return view;
    }

    private WebView createWebView(final ServerProfile profile) {
        WebView view = new WebView(this);
        WebSettings settings = view.getSettings();
        settings.setJavaScriptEnabled(true);
        // 前端把 token 存在 cookie、栏目选择存在 localStorage，都要 DOM storage
        settings.setDomStorageEnabled(true);
        settings.setDatabaseEnabled(true);
        settings.setUseWideViewPort(true);
        settings.setLoadWithOverviewMode(true);
        settings.setSupportZoom(false);
        settings.setBuiltInZoomControls(false);
        // 服务端是局域网明文 HTTP，这里必须放行（否则 WebView 会当成不安全混合内容拦掉）
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_ALWAYS_ALLOW);
        settings.setCacheMode(WebSettings.LOAD_DEFAULT);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        // 让服务端日志/前端能认出这是 App 客户端而不是普通浏览器
        settings.setUserAgentString(settings.getUserAgentString() + " AnyBootRemote/1.1");

        CookieManager.getInstance().setAcceptCookie(true);

        view.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView target, WebResourceRequest request) {
                Uri uri = request.getUrl();
                String scheme = uri == null ? null : uri.getScheme();
                if ("http".equalsIgnoreCase(scheme) || "https".equalsIgnoreCase(scheme)) {
                    // 站内跳转（含 SPA 路由）留在 WebView 里
                    return false;
                }
                // http(s) 之外（mailto:、tel: 等）交给系统
                try {
                    startActivity(new Intent(Intent.ACTION_VIEW, uri));
                } catch (Exception ignored) {
                    // 没有对应应用就静默忽略，不能因为外链让客户端崩掉
                }
                return true;
            }

            @Override
            public void onPageStarted(WebView target, String url, Bitmap favicon) {
                Tab tab = tabs.get(profile.id);
                if (tab != null) {
                    tab.mainFailed = false;
                    setStatus(tab, STATUS_LOADING);
                }
                if (isActive(profile.id)) {
                    hideError();
                    showLoading(true);
                }
            }

            @Override
            public void onPageFinished(WebView target, String url) {
                Tab tab = tabs.get(profile.id);
                if (tab != null && tab.status != STATUS_FAIL) {
                    setStatus(tab, STATUS_OK);
                }
                if (isActive(profile.id)) {
                    showLoading(false);
                }
            }

            @Override
            public void onReceivedError(WebView target, WebResourceRequest request,
                                        WebResourceError error) {
                // 只有主文档失败才算「连不上」；子资源（图标、字体）失败不该盖住页面
                if (request == null || !request.isForMainFrame()) {
                    return;
                }
                Tab tab = tabs.get(profile.id);
                if (tab != null) {
                    tab.mainFailed = true;
                    setStatus(tab, STATUS_FAIL);
                }
                if (isActive(profile.id)) {
                    String description = error == null ? "" : String.valueOf(error.getDescription());
                    showError(tab == null ? profile : tab.profile, description);
                }
            }
        });
        return view;
    }

    // ------------------------------------------------------------------
    // 标签栏
    // ------------------------------------------------------------------

    private void renderTabs() {
        tabStrip.removeAllViews();
        chips.clear();
        dots.clear();
        for (ServerProfile profile : servers) {
            LinearLayout chip = buildChip(profile);
            chips.put(profile.id, chip);
            tabStrip.addView(chip);
        }
        refreshChips();
    }

    private LinearLayout buildChip(final ServerProfile profile) {
        LinearLayout chip = new LinearLayout(this);
        chip.setOrientation(LinearLayout.HORIZONTAL);
        chip.setGravity(Gravity.CENTER_VERTICAL);
        chip.setPadding(dp(10), dp(5), dp(12), dp(5));
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, dp(32));
        params.setMargins(0, 0, dp(6), 0);
        chip.setLayoutParams(params);

        View dot = new View(this);
        LinearLayout.LayoutParams dotParams = new LinearLayout.LayoutParams(dp(8), dp(8));
        dotParams.setMargins(0, 0, dp(6), 0);
        dot.setLayoutParams(dotParams);
        dot.setBackground(oval(statusColor(STATUS_UNKNOWN)));
        chip.addView(dot);
        dots.put(profile.id, dot);

        TextView label = new TextView(this);
        label.setText(profile.label());
        label.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13);
        label.setSingleLine(true);
        label.setTag("label");
        chip.addView(label);

        chip.setOnClickListener(v -> activate(profile.id));
        chip.setOnLongClickListener(v -> {
            showEditDialog(ServerStore.find(servers, profile.id), false);
            return true;
        });
        return chip;
    }

    private void refreshChips() {
        for (ServerProfile profile : servers) {
            applyChipAppearance(profile);
        }
    }

    private void applyChipAppearance(ServerProfile profile) {
        LinearLayout chip = chips.get(profile.id);
        if (chip == null) {
            return;
        }
        boolean active = isActive(profile.id);
        int border = profile.group != null && !profile.group.trim().isEmpty()
                ? profile.groupColor()
                : CHIP_BORDER;
        chip.setBackground(rounded(active ? ACCENT : CHIP_BG, active ? ACCENT : border, 16));
        View label = chip.findViewWithTag("label");
        if (label instanceof TextView) {
            ((TextView) label).setText(profile.label());
            ((TextView) label).setTextColor(active ? Color.WHITE : CHIP_TEXT);
        }
        View dot = dots.get(profile.id);
        if (dot != null) {
            Tab tab = tabs.get(profile.id);
            dot.setBackground(oval(statusColor(tab == null ? STATUS_UNKNOWN : tab.status)));
        }
    }

    private void scrollToChip(String id) {
        final LinearLayout chip = chips.get(id);
        if (chip == null) {
            return;
        }
        tabScroll.post(() -> tabScroll.smoothScrollTo(
                Math.max(0, chip.getLeft() - dp(12)), 0));
    }

    private void setStatus(Tab tab, int status) {
        tab.status = status;
        View dot = dots.get(tab.profile.id);
        if (dot != null) {
            dot.setBackground(oval(statusColor(status)));
        }
    }

    private int statusColor(int status) {
        switch (status) {
            case STATUS_LOADING:
                return COLOR_LOADING;
            case STATUS_OK:
                return COLOR_OK;
            case STATUS_FAIL:
                return COLOR_FAIL;
            default:
                return COLOR_UNKNOWN;
        }
    }

    // ------------------------------------------------------------------
    // 连接 / 切换
    // ------------------------------------------------------------------

    private boolean isActive(String id) {
        return id != null && id.equals(activeId);
    }

    private Tab ensureTab(ServerProfile profile) {
        Tab tab = tabs.get(profile.id);
        if (tab == null) {
            tab = new Tab();
            tab.profile = profile;
            tabs.put(profile.id, tab);
        }
        tab.profile = profile;
        if (tab.view == null) {
            tab.view = createWebView(profile);
            tab.view.setVisibility(View.GONE);
            container.addView(tab.view, 0, new FrameLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        }
        return tab;
    }

    /** 保证这个服务端的 WebView 存在并加载（多个标签可以同时在线） */
    private void connect(ServerProfile profile, boolean forceReload) {
        Tab tab = ensureTab(profile);
        if (!forceReload && tab.started) {
            // 已经连过就保留后台页面，切回来直接显示，不重新拉
            return;
        }
        tab.mainFailed = false;
        tab.started = true;
        setStatus(tab, STATUS_LOADING);
        tab.view.loadUrl(profile.url());
    }

    private void activate(String id) {
        ServerProfile profile = ServerStore.find(servers, id);
        if (profile == null) {
            return;
        }
        activeId = id;
        ServerStore.setActiveId(this, id);
        // 先确保这个服务端的 WebView 在（可能是第一次点它），再统一按 activeId 定显隐
        connect(profile, false);
        applyVisibility();
        refreshChips();
        scrollToChip(id);
        Tab tab = tabs.get(id);
        if (tab != null && tab.status == STATUS_FAIL) {
            showError(profile, "上一次加载失败");
        } else {
            hideError();
            showLoading(tab != null && tab.status == STATUS_LOADING);
        }
    }

    /** 「启动时连接全部」：把每个服务端都拉起来，标签上能同时看到它们的状态 */
    private void connectAllServers() {
        for (ServerProfile profile : servers) {
            connect(profile, false);
        }
        applyVisibility();
        Tab active = tabs.get(activeId);
        if (active != null && active.status != STATUS_FAIL) {
            hideError();
        }
    }

    private void applyVisibility() {
        for (Tab tab : tabs.values()) {
            if (tab.view != null) {
                tab.view.setVisibility(isActive(tab.profile.id) ? View.VISIBLE : View.GONE);
            }
        }
    }

    private void reloadActive() {
        ServerProfile profile = ServerStore.find(servers, activeId);
        if (profile == null) {
            return;
        }
        Tab tab = ensureTab(profile);
        tab.mainFailed = false;
        tab.started = true;
        hideError();
        showLoading(true);
        setStatus(tab, STATUS_LOADING);
        tab.view.loadUrl(profile.url());
    }

    /** 全部重载 = 把没连上的也连上，并把在线的一起刷新 */
    private void reloadAll() {
        for (ServerProfile profile : servers) {
            Tab tab = ensureTab(profile);
            tab.mainFailed = false;
            tab.started = true;
            setStatus(tab, STATUS_LOADING);
            tab.view.loadUrl(profile.url());
        }
        activate(activeId);
    }

    private void dropTab(ServerProfile profile) {
        Tab tab = tabs.remove(profile.id);
        if (tab != null && tab.view != null) {
            container.removeView(tab.view);
            tab.view.destroy();
        }
    }

    private void showLoading(boolean visible) {
        progress.setVisibility(visible ? View.VISIBLE : View.GONE);
    }

    private void showError(ServerProfile profile, String description) {
        showLoading(false);
        for (Tab tab : tabs.values()) {
            if (tab.view != null) {
                tab.view.setVisibility(View.GONE);
            }
        }
        errorText.setText("服务端：" + (profile == null ? "" : profile.label())
                + "\n地址：" + (profile == null ? "" : profile.host)
                + "\n\n" + description
                + "\n\n请确认：\n"
                + "· 电脑上 AnyBoot 的「远程控制」已打开，手机和电脑在同一局域网\n"
                + "· 地址填的是电脑上的 IP 和端口（例如 192.168.0.120:12300）");
        errorPanel.setVisibility(View.VISIBLE);
    }

    private void hideError() {
        errorPanel.setVisibility(View.GONE);
        applyVisibility();
    }

    // ------------------------------------------------------------------
    // 服务端的增删改查
    // ------------------------------------------------------------------

    /**
     * 新增（profile == null）或编辑一个服务端。
     *
     * @param firstRun 首次使用：对话框不可取消，且不给「取消」按钮
     */
    private void showEditDialog(final ServerProfile profile, final boolean firstRun) {
        final boolean editing = profile != null;

        final AutoCompleteTextView groupInput = new AutoCompleteTextView(this);
        groupInput.setSingleLine(true);
        groupInput.setHint("例如：书房台式机（同一台电脑的多个 IP 填同一个分组）");
        groupInput.setInputType(InputType.TYPE_CLASS_TEXT);
        List<String> groups = ServerStore.knownGroups(servers);
        if (!groups.isEmpty()) {
            groupInput.setAdapter(new ArrayAdapter<>(this,
                    android.R.layout.simple_dropdown_item_1line, groups));
        }
        groupInput.setText(editing ? profile.group : "");

        final EditText nameInput = new EditText(this);
        nameInput.setSingleLine(true);
        nameInput.setHint("例如：有线网卡 / 5G WiFi（可留空，用地址兜底）");
        nameInput.setInputType(InputType.TYPE_CLASS_TEXT);
        nameInput.setText(editing ? profile.name : "");

        final EditText hostInput = new EditText(this);
        hostInput.setSingleLine(true);
        hostInput.setHint("192.168.0.120:12300 或整条带 token 的地址");
        hostInput.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        hostInput.setText(editing ? profile.host : "");

        final EditText tokenInput = new EditText(this);
        tokenInput.setSingleLine(true);
        tokenInput.setHint("可留空，页面上也能填");
        tokenInput.setInputType(InputType.TYPE_CLASS_TEXT
                | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD);
        tokenInput.setText(editing ? profile.token : "");

        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setPadding(dp(20), dp(8), dp(20), 0);
        box.addView(hint("分组（用来区分不同的电脑）"));
        box.addView(groupInput);
        box.addView(hint("服务端名（同一台电脑的不同网卡/地址，用名字区分）"));
        box.addView(nameInput);
        box.addView(hint("服务端地址（电脑端「远程控制」里显示的地址）"));
        box.addView(hostInput);
        box.addView(hint("访问令牌 token"));
        box.addView(tokenInput);

        AlertDialog.Builder builder = new AlertDialog.Builder(this);
        builder.setTitle(editing ? "编辑服务端" : (firstRun ? "连接 AnyBoot 服务端" : "添加服务端"));
        builder.setView(box);
        builder.setCancelable(!firstRun);
        builder.setPositiveButton("保存", null);
        if (!firstRun) {
            builder.setNegativeButton("取消", null);
        }
        if (editing) {
            builder.setNeutralButton("删除", null);
        }

        final AlertDialog dialog = builder.create();
        dialog.setOnShowListener(ignored -> {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(v -> {
                String rawHost = hostInput.getText().toString();
                String host = ServerProfile.normalizeHost(rawHost);
                if (host.isEmpty()) {
                    Toast.makeText(this, "请填服务端地址", Toast.LENGTH_SHORT).show();
                    return;
                }
                String token = tokenInput.getText().toString().trim();
                if (token.isEmpty()) {
                    // 整条 URL 粘进来的情况
                    token = ServerProfile.tokenFromUrl(rawHost);
                }
                String name = nameInput.getText().toString().trim();
                ServerProfile target = editing ? profile : new ServerProfile();
                target.group = groupInput.getText().toString().trim();
                target.name = name.isEmpty() ? host : name;
                target.host = host;
                target.token = token;
                if (!editing) {
                    servers.add(target);
                }
                ServerStore.sort(servers);
                ServerStore.save(this, servers);
                renderTabs();
                dialog.dismiss();
                if (editing) {
                    // 改了地址/token 就按新配置重连这个服务端
                    connect(target, true);
                    activate(target.id);
                } else {
                    activate(target.id);
                }
            });
            if (editing) {
                dialog.getButton(AlertDialog.BUTTON_NEUTRAL)
                        .setOnClickListener(v -> confirmDelete(profile, dialog));
            }
        });
        dialog.show();

        if (firstRun || !editing) {
            hostInput.requestFocus();
        }
    }

    private void confirmDelete(final ServerProfile profile, final AlertDialog parent) {
        new AlertDialog.Builder(this)
                .setTitle("删除服务端")
                .setMessage("确定删除「" + profile.label() + "」吗？只会删掉手机上的这条记录。")
                .setPositiveButton("删除", (dialog, which) -> {
                    parent.dismiss();
                    servers.remove(profile);
                    dropTab(profile);
                    ServerStore.save(this, servers);
                    renderTabs();
                    if (servers.isEmpty()) {
                        activeId = "";
                        ServerStore.setActiveId(this, "");
                        showEditDialog(null, true);
                        return;
                    }
                    if (isActive(profile.id)) {
                        activeId = servers.get(0).id;
                    }
                    activate(activeId);
                })
                .setNegativeButton("取消", null)
                .show();
    }

    private void showServerList() {
        if (servers.isEmpty()) {
            showEditDialog(null, false);
            return;
        }
        final String[] labels = new String[servers.size()];
        for (int i = 0; i < servers.size(); i++) {
            ServerProfile profile = servers.get(i);
            labels[i] = (isActive(profile.id) ? "● " : "　") + profile.label()
                    + "　" + profile.host;
        }
        new AlertDialog.Builder(this)
                .setTitle("切换服务端（长按标签也能编辑）")
                .setItems(labels, (dialog, which) -> activate(servers.get(which).id))
                .setPositiveButton("添加", (dialog, which) -> showEditDialog(null, false))
                .setNegativeButton("关闭", null)
                .show();
    }

    private TextView hint(String text) {
        TextView view = new TextView(this);
        view.setText(text);
        view.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13);
        view.setPadding(0, dp(10), 0, dp(2));
        return view;
    }

    // ------------------------------------------------------------------
    // 菜单 / 生命周期 / 返回键
    // ------------------------------------------------------------------

    @Override
    public boolean onCreateOptionsMenu(Menu menu) {
        menu.add(0, 1, 0, "添加服务端");
        menu.add(0, 2, 1, "编辑当前服务端");
        menu.add(0, 3, 2, "服务端列表");
        menu.add(0, 4, 3, "重载当前页面");
        menu.add(0, 5, 4, "全部重载");
        MenuItem auto = menu.add(0, 6, 5, "启动时连接全部服务端");
        auto.setCheckable(true);
        auto.setChecked(connectAll);
        return true;
    }

    @Override
    public boolean onOptionsItemSelected(MenuItem item) {
        switch (item.getItemId()) {
            case 1:
                showEditDialog(null, false);
                return true;
            case 2:
                showEditDialog(ServerStore.find(servers, activeId), false);
                return true;
            case 3:
                showServerList();
                return true;
            case 4:
                reloadActive();
                return true;
            case 5:
                reloadAll();
                return true;
            case 6:
                connectAll = !connectAll;
                ServerStore.setConnectAll(this, connectAll);
                item.setChecked(connectAll);
                if (connectAll) {
                    connectAllServers();
                }
                Toast.makeText(this, connectAll
                        ? "已开启：启动时把所有服务端一起连上"
                        : "已关闭：只连当前标签", Toast.LENGTH_SHORT).show();
                return true;
            default:
                return super.onOptionsItemSelected(item);
        }
    }

    @Override
    public boolean onPrepareOptionsMenu(Menu menu) {
        MenuItem auto = menu.findItem(6);
        if (auto != null) {
            auto.setChecked(connectAll);
        }
        return super.onPrepareOptionsMenu(menu);
    }

    @Override
    protected void onResume() {
        super.onResume();
        for (Tab tab : tabs.values()) {
            if (tab.view != null) {
                tab.view.onResume();
            }
        }
    }

    @Override
    protected void onPause() {
        // 后台标签也一起暂停 JS/定时器，回前台再恢复
        for (Tab tab : tabs.values()) {
            if (tab.view != null) {
                tab.view.onPause();
            }
        }
        super.onPause();
    }

    @Override
    public void onBackPressed() {
        // 当前页自己能退就让它退（弹窗、页内路由），退不动再交给系统
        Tab tab = tabs.get(activeId);
        if (tab != null && tab.view != null && tab.view.canGoBack()) {
            tab.view.goBack();
            return;
        }
        super.onBackPressed();
    }

    // ------------------------------------------------------------------
    // 绘制杂项
    // ------------------------------------------------------------------

    private GradientDrawable oval(int color) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setShape(GradientDrawable.OVAL);
        drawable.setColor(color);
        return drawable;
    }

    private GradientDrawable rounded(int fill, int stroke, int radiusDp) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setShape(GradientDrawable.RECTANGLE);
        drawable.setCornerRadius(dp(radiusDp));
        drawable.setColor(fill);
        if (stroke != 0) {
            drawable.setStroke(dp(1), stroke);
        }
        return drawable;
    }

    private int dp(int value) {
        return Math.round(TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value,
                getResources().getDisplayMetrics()));
    }
}
