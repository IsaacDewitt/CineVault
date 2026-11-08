package com.cinevault.mobile;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.util.TypedValue;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.inputmethod.EditorInfo;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * CineVault 手机端：纯原生 Android，只做三件事——
 * 浏览电脑上的影视记录、搜索、把播放地址交给 VLC。
 * 不维护第二份数据库，不写回观看记录，不内嵌播放器。
 */
public class MainActivity extends Activity {
    private static final int C_BG0 = 0xFF0A0F1A;
    private static final int C_BG1 = 0xFF111A2B;
    private static final int C_BG2 = 0xFF18233A;
    private static final int C_BG3 = 0xFF212E49;
    private static final int C_TEXT1 = 0xFFE6ECF7;
    private static final int C_TEXT2 = 0xFFA3B3CD;
    private static final int C_TEXT3 = 0xFF7D8CAB;
    private static final int C_ACCENT = 0xFFF0B429;
    private static final int C_ACCENT_INK = 0xFF1A1406;
    private static final int C_GREEN = 0xFF34D399;
    private static final int C_RED = 0xFFF87171;
    private static final int C_PURPLE = 0xFFA78BFA;

    private static final String PREFS = "cinevault";
    private static final String P_BASE = "base_url";
    private static final String P_TOKEN = "token";

    private static final String[] TAB_LABELS = {"剧集", "电影", "历史"};

    private SharedPreferences prefs;
    private ExecutorService io;
    private final Handler ui = new Handler(Looper.getMainLooper());

    private String baseUrl = "";
    private String token = "";
    private String query = "";
    private int currentTab = 0;
    private volatile int generation = 0;
    private volatile int connectionGeneration = 0;
    private volatile int overviewGeneration = 0;
    private volatile boolean destroyed = false;
    private volatile ApiClient listClient;
    private volatile ApiClient overviewClient;
    private volatile ApiClient detailClient;
    private volatile ApiClient connectClient;
    private volatile ApiClient mediaClient;
    private volatile boolean playPending = false;
    private int moviePage = 0;
    private int seriesPage = 0;
    private boolean inDetail = false;
    private long detailId = 0;
    private String detailName = "";
    private Runnable searchRunnable;

    private LinearLayout listBox;
    private TextView statsView;
    private EditText searchView;
    private final List<TextView> tabViews = new ArrayList<>();

    private int sidePad;
    private float density;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        density = getResources().getDisplayMetrics().density;
        int screenWidth = getResources().getDisplayMetrics().widthPixels;
        sidePad = Math.max(dp(16), (screenWidth - dp(860)) / 2);

        prefs = getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        baseUrl = prefs.getString(P_BASE, "");
        token = prefs.getString(P_TOKEN, "");
        io = Executors.newFixedThreadPool(3);

        // 旋转/分屏会重建 Activity（Manifest 不拦 configChanges），恢复页签、搜索与详情位置。
        if (savedInstanceState != null) {
            currentTab = savedInstanceState.getInt("tab", 0);
            query = savedInstanceState.getString("query", "");
            detailId = savedInstanceState.getLong("detail_id", 0);
            detailName = savedInstanceState.getString("detail_name", "");
        }

        styleWindow();
        if (token.isEmpty()) {
            showConnect();
        } else if (detailId > 0) {
            openSeries(detailId, detailName);
        } else {
            showLibrary();
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle outState) {
        outState.putInt("tab", currentTab);
        outState.putString("query", query);
        outState.putLong("detail_id", detailId);
        outState.putString("detail_name", detailName);
        super.onSaveInstanceState(outState);
    }

    @Override
    protected void onDestroy() {
        destroyed = true;
        generation++;
        connectionGeneration++;
        cancelPendingSearch();
        cancelAllRequests();
        ui.removeCallbacksAndMessages(null);
        if (io != null) io.shutdownNow();
        super.onDestroy();
    }

    @Override
    public void onBackPressed() {
        if (inDetail) {
            showLibrary();
            return;
        }
        super.onBackPressed();
    }

    private void styleWindow() {
        Window w = getWindow();
        w.setStatusBarColor(C_BG0);
        w.setNavigationBarColor(C_BG0);
        w.getDecorView().setSystemUiVisibility(
                View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR & ~View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR
        );
    }

    // ============================================================
    // 配对 / 连接页
    // ============================================================

    private void showConnect() {
        cancelPendingSearch();
        cancelAllRequests();
        inDetail = false;
        detailId = 0;
        detailName = "";
        generation++;
        connectionGeneration++;
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(C_BG0);

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        root.addView(scroll, match());

        LinearLayout page = column();
        page.setPadding(sidePad, dp(18), sidePad, dp(32));
        scroll.addView(page, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
        ));

        if (!token.isEmpty()) {
            TextView back = actionChip("‹ 返回影库");
            back.setOnClickListener(v -> showLibrary());
            page.addView(back, wrap());
            space(page, 16);
        }

        page.addView(brandBlock(false), match());
        space(page, 28);

        TextView title = text("连接电脑", 27, C_TEXT1, true);
        page.addView(title);
        TextView subtitle = text(
                "CineVault 在电脑上运行，手机端只读取记录。\n"
                        + "所有播放都交给 VLC，原片直读，不转码。",
                13, C_TEXT3, false
        );
        subtitle.setLineSpacing(dp(4), 1f);
        page.addView(subtitle, marginTop(dp(7)));
        space(page, 22);

        LinearLayout card = card();
        card.setPadding(dp(16), dp(17), dp(16), dp(17));
        page.addView(card, match());

        TextView serverLabel = fieldLabel("电脑地址");
        card.addView(serverLabel);
        EditText server = input("例如 192.168.1.10:42337");
        server.setText(baseUrl);
        server.setInputType(InputType.TYPE_TEXT_VARIATION_URI);
        server.setSingleLine(true);
        card.addView(server, marginTop(dp(7)));

        space(card, 15);
        TextView codeLabel = fieldLabel("配对码");
        card.addView(codeLabel);
        EditText code = input("电脑端「手机观看」显示的 8 位数字");
        code.setInputType(InputType.TYPE_CLASS_NUMBER);
        code.setSingleLine(true);
        code.setImeOptions(EditorInfo.IME_ACTION_DONE);
        card.addView(code, marginTop(dp(7)));

        space(card, 18);
        TextView status = text("", 12, C_TEXT3, false);
        status.setVisibility(View.GONE);
        status.setLineSpacing(dp(3), 1f);
        card.addView(status, marginBottom(dp(10)));

        TextView connect = primaryButton("配对并进入影库");
        card.addView(connect, match());
        space(card, 9);
        TextView discover = secondaryButton("自动发现电脑");
        card.addView(discover, match());

        if (!token.isEmpty()) {
            space(card, 9);
            TextView useSaved = ghostButton("用已保存的连接直接进入");
            useSaved.setOnClickListener(v -> {
                String b = server.getText().toString();
                connectWithToken(b, status, useSaved);
            });
            card.addView(useSaved, match());
        }

        space(page, 18);
        infoNote(page,
                "连接前提",
                "电脑端 CineVault 必须正在运行，并在侧栏打开「手机观看」。"
                        + "首次连接时 Windows 可能询问防火墙权限，请允许专用网络访问。");

        space(page, 12);
        infoNote(page,
                "只读边界",
                "手机端不能修改评分、标签或观看进度，也不能删除和扫描。"
                        + "手机播放不会写回 CineVault，观看记录仍以电脑端为准。");

        space(page, 12);
        infoNote(page,
                "网络安全",
                "本功能只面向可信的家庭/专用局域网，当前链路未启用 TLS。"
                        + "不要在公共 Wi-Fi 配对；不确定网络是否可信时请关闭电脑端的「手机观看」。");

        if (!token.isEmpty()) {
            space(page, 20);
            TextView clear = ghostButton("清除已保存的配对");
            clear.setTextColor(C_RED);
            clear.setOnClickListener(v -> new AlertDialog.Builder(this)
                    .setTitle("清除配对")
                    .setMessage("只会清除手机端保存的地址和令牌，不会影响电脑上的 CineVault 数据。")
                    .setPositiveButton("清除", (d, w) -> {
                        if (!isUsable()) return;
                        generation++;
                        connectionGeneration++;
                        cancelPendingSearch();
                        prefs.edit().clear().apply();
                        baseUrl = "";
                        token = "";
                        showConnect();
                    })
                    .setNegativeButton("取消", null)
                    .show());
            page.addView(clear, match());
        }

        connect.setOnClickListener(v -> pairAndEnter(
                server.getText().toString(), code.getText().toString(), status, connect, discover
        ));
        discover.setOnClickListener(v -> autoDiscover(server, code, status, connect, discover));

        setContentView(root);
    }

    private void ackStatus(TextView view, String msg, int color) {
        view.setVisibility(View.VISIBLE);
        view.setText(msg);
        view.setTextColor(color);
    }

    private void pairAndEnter(String serverRaw, String code, TextView status,
                              TextView connect, TextView discover) {
        String base;
        try {
            base = ApiClient.normalizeBase(serverRaw);
        } catch (Exception e) {
            ackStatus(status, e.getMessage(), C_RED);
            return;
        }
        if (code.trim().isEmpty()) {
            if (!token.isEmpty()) {
                connectWithToken(serverRaw, status, connect);
            } else {
                ackStatus(status, "请输入电脑端显示的 8 位配对码；电脑端未开启时请先打开「手机观看」。", C_RED);
            }
            return;
        }
        String pairingCode = code.trim();
        if (!pairingCode.matches("[0-9]{8}")) {
            ackStatus(status, "请输入电脑端显示的 8 位数字配对码。", C_RED);
            return;
        }
        connect.setEnabled(false);
        discover.setEnabled(false);
        ackStatus(status, "正在配对…", C_TEXT2);
        final int my = generation;
        final int connection = ++connectionGeneration;
        cancelConnectRequest();
        final ApiClient client = new ApiClient(base, "");
        connectClient = client;
        executeIo(() -> {
            if (!isConnectionActive(my, connection)) return;
            try {
                JSONObject r = client.pairWith(pairingCode);
                String newToken = r.getString("token");
                ui.post(() -> {
                    if (!isConnectionActive(my, connection)) return;
                    prefs.edit().putString(P_BASE, base).putString(P_TOKEN, newToken).apply();
                    baseUrl = base;
                    token = newToken;
                    showLibrary();
                });
            } catch (Exception e) {
                ui.post(() -> {
                    if (!isConnectionActive(my, connection)) return;
                    connect.setEnabled(true);
                    discover.setEnabled(true);
                    ackStatus(status, friendly(e), C_RED);
                });
            }
        });
    }

    private void connectWithToken(String serverRaw, TextView status, TextView button) {
        String base;
        try {
            base = ApiClient.normalizeBase(serverRaw);
        } catch (Exception e) {
            ackStatus(status, e.getMessage(), C_RED);
            return;
        }
        if (token.isEmpty()) {
            ackStatus(status, "还没有配对令牌，请输入配对码。", C_RED);
            return;
        }
        button.setEnabled(false);
        ackStatus(status, "正在连接…", C_TEXT2);
        final int my = generation;
        final int connection = ++connectionGeneration;
        final String savedToken = token;
        cancelConnectRequest();
        final ApiClient client = new ApiClient(base, savedToken);
        connectClient = client;
        executeIo(() -> {
            if (!isConnectionActive(my, connection)) return;
            try {
                client.get("/api/v1/overview");
                ui.post(() -> {
                    if (!isConnectionActive(my, connection)) return;
                    prefs.edit().putString(P_BASE, base).putString(P_TOKEN, savedToken).apply();
                    baseUrl = base;
                    showLibrary();
                });
            } catch (Exception e) {
                ui.post(() -> {
                    if (!isConnectionActive(my, connection)) return;
                    button.setEnabled(true);
                    ackStatus(status, friendly(e), C_RED);
                });
            }
        });
    }

    private void autoDiscover(EditText server, EditText code, TextView status,
                              TextView connect, TextView discover) {
        String pairingCode = code.getText().toString().trim();
        if (!pairingCode.matches("[0-9]{8}")) {
            ackStatus(status, "自动发现需要先输入电脑端显示的 8 位数字配对码。", C_RED);
            code.requestFocus();
            return;
        }
        discover.setEnabled(false);
        connect.setEnabled(false);
        ackStatus(status, "正在通过配对码校验局域网内的 CineVault…", C_TEXT2);
        final int my = generation;
        final int connection = ++connectionGeneration;
        executeIo(() -> {
            if (!isConnectionActive(my, connection)) return;
            try {
                String found = LanDiscovery.discover(pairingCode);
                ui.post(() -> {
                    if (!isConnectionActive(my, connection)) return;
                    server.setText(found);
                    discover.setEnabled(true);
                    connect.setEnabled(true);
                    ackStatus(status, "已验证 " + found + " 持有当前配对码。点击配对并进入影库。", C_GREEN);
                    connect.requestFocus();
                });
            } catch (Exception e) {
                ui.post(() -> {
                    if (!isConnectionActive(my, connection)) return;
                    discover.setEnabled(true);
                    connect.setEnabled(true);
                    ackStatus(status,
                            friendly(e) + "\n也可以手动填写电脑地址。",
                            C_RED);
                });
            }
        });
    }

    // ============================================================
    // 影库主页
    // ============================================================

    private void showLibrary() {
        cancelPendingSearch();
        cancelAllRequests();
        inDetail = false;
        detailId = 0;
        detailName = "";
        generation++;
        prefs.edit().putString(P_BASE, baseUrl).putString(P_TOKEN, token).apply();

        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(C_BG0);
        LinearLayout page = column();
        page.setPadding(sidePad, 0, sidePad, 0);
        root.addView(page, match());

        page.addView(header(), wrap());

        LinearLayout tools = column();
        tools.setPadding(0, dp(4), 0, dp(10));
        page.addView(tools, match());

        statsView = text("正在读取影库…", 11, C_TEXT3, false);
        statsView.setLetterSpacing(0.02f);
        tools.addView(statsView, marginBottom(dp(9)));

        searchView = input("搜索标题、剧集、标签…");
        searchView.setSingleLine(true);
        searchView.setImeOptions(EditorInfo.IME_ACTION_SEARCH);
        searchView.setText(query);
        searchView.setSelection(searchView.getText().length());
        tools.addView(searchView, match());
        final EditText searchTarget = searchView;
        searchView.addTextChangedListener(new TextWatcher() {
            @Override public void beforeTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void onTextChanged(CharSequence s, int a, int b, int c) {}
            @Override public void afterTextChanged(Editable s) {
                if (!isUsable() || searchTarget != searchView) return;
                query = s.toString();
                cancelPendingSearch();
                final int scheduledGeneration = generation;
                searchRunnable = () -> {
                    if (!isActive(scheduledGeneration) || searchTarget != searchView) return;
                    searchRunnable = null;
                    reload();
                };
                ui.postDelayed(searchRunnable, 330);
            }
        });

        space(tools, 11);
        LinearLayout tabs = new LinearLayout(this);
        tabs.setOrientation(LinearLayout.HORIZONTAL);
        tabViews.clear();
        for (int i = 0; i < TAB_LABELS.length; i++) {
            final int index = i;
            TextView tab = text(TAB_LABELS[i], 13, C_TEXT2, true);
            tab.setGravity(Gravity.CENTER);
            tab.setPadding(0, dp(9), 0, dp(9));
            tab.setOnClickListener(v -> {
                if (currentTab == index) return;
                cancelPendingSearch();
                currentTab = index;
                styleTabs();
                updateSearchHint();
                reload();
            });
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, dp(38), 1f);
            if (i > 0) lp.leftMargin = dp(7);
            tabs.addView(tab, lp);
            tabViews.add(tab);
        }
        tools.addView(tabs, match());
        styleTabs();

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.setClipToPadding(false);
        page.addView(scroll, new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f
        ));
        listBox = column();
        listBox.setPadding(0, dp(4), 0, dp(30));
        scroll.addView(listBox, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
        ));

        setContentView(root);
        loadOverview();
        reload();
    }

    private View header() {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, dp(15), 0, dp(14));

        LinearLayout logo = column();
        LinearLayout titleRow = new LinearLayout(this);
        titleRow.setOrientation(LinearLayout.HORIZONTAL);
        titleRow.setGravity(Gravity.CENTER_VERTICAL);
        TextView title = text("CineVault", 21, C_TEXT1, true);
        title.setLetterSpacing(0.01f);
        titleRow.addView(title);
        TextView badge = text("只读", 9, C_ACCENT, true);
        badge.setLetterSpacing(0.08f);
        badge.setGravity(Gravity.CENTER);
        badge.setPadding(dp(7), dp(2), dp(7), dp(2));
        badge.setBackground(roundRect(0x22F0B429, dp(5), 0x44F0B429, 1));
        LinearLayout.LayoutParams badgeLp = wrap();
        badgeLp.leftMargin = dp(8);
        titleRow.addView(badge, badgeLp);
        logo.addView(titleRow);
        TextView sub = text("局域网影视库", 10, C_TEXT3, false);
        sub.setLetterSpacing(0.12f);
        logo.addView(sub, marginTop(dp(1)));
        row.addView(logo, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));

        TextView refresh = actionChip("↻");
        refresh.setContentDescription("刷新");
        refresh.setOnClickListener(v -> {
            loadOverview();
            reload();
        });
        row.addView(refresh, wrap());

        TextView server = actionChip("服务器");
        LinearLayout.LayoutParams slp = wrap();
        slp.leftMargin = dp(6);
        server.setOnClickListener(v -> showConnect());
        row.addView(server, slp);
        return row;
    }

    private void styleTabs() {
        for (int i = 0; i < tabViews.size(); i++) {
            TextView t = tabViews.get(i);
            if (i == currentTab) {
                t.setTextColor(C_ACCENT);
                t.setBackground(roundRect(0x22F0B429, dp(8), 0x44F0B429, 1));
            } else {
                t.setTextColor(C_TEXT2);
                t.setBackground(roundRect(C_BG1, dp(8), 0x22FFFFFF, 1));
            }
        }
    }

    private void updateSearchHint() {
        if (searchView == null) return;
        if (currentTab == 2) {
            searchView.setHint("在本机加载的历史中筛选…");
        } else if (currentTab == 1) {
            searchView.setHint("搜索电影标题、文件名、标签…");
        } else {
            searchView.setHint("搜索剧集名称、标签…");
        }
    }

    private void loadOverview() {
        // 这里不能复用 generation：reload() 会递增它，概览响应回来时会被误判成过期。
        // 用控件实例本身作为代际标记——页面一旦重建，旧 statsView 不再对应当前界面。
        final TextView target = statsView;
        if (target == null) return;
        final int connection = connectionGeneration;
        final int overview = ++overviewGeneration;
        cancelOverviewRequest();
        final ApiClient client = new ApiClient(baseUrl, token);
        overviewClient = client;
        executeIo(() -> {
            if (!isUsable() || connection != connectionGeneration
                    || overview != overviewGeneration) return;
            try {
                JSONObject r = client.get("/api/v1/overview");
                JSONObject stats = r.optJSONObject("stats");
                if (stats == null) return;
                long videos = stats.optLong("total_videos");
                long series = stats.optLong("total_series");
                long movies = stats.optLong("movie_count");
                String s = String.format(Locale.CHINA, "%d 部剧集 · %d 部电影 · %d 个视频",
                        series, movies, videos);
                ui.post(() -> {
                    if (isUsable() && connection == connectionGeneration
                                && overview == overviewGeneration && target == statsView) {
                        target.setText(s);
                    }
                });
            } catch (Exception ignored) {
                ui.post(() -> {
                    if (isUsable() && connection == connectionGeneration
                                && overview == overviewGeneration && target == statsView) {
                        target.setText("影库概览暂不可用");
                    }
                });
            }
        });
    }

    private void reload() {
        cancelPendingSearch();
        if (listBox == null || !isUsable()) return;
        moviePage = 0;
        seriesPage = 0;
        final int my = ++generation;
        final int tab = currentTab;
        final LinearLayout target = listBox;
        final String q = query.trim();
        cancelListRequest();
        final ApiClient client = new ApiClient(baseUrl, token);
        listClient = client;
        target.removeAllViews();
        target.addView(loadingRow());

        executeIo(() -> {
            if (!isActive(my)) return;
            try {
                JSONObject r;
                if (tab == 0) {
                    String path = "/api/v1/series?sort=recent&limit=40&page=0";
                    if (!q.isEmpty()) path += "&q=" + ApiClient.urlEncode(q);
                    r = client.get(path);
                } else if (tab == 1) {
                    String path = "/api/v1/videos?kind=movie&limit=40&page=0";
                    if (!q.isEmpty()) path += "&q=" + ApiClient.urlEncode(q);
                    r = client.get(path);
                } else {
                    r = client.get("/api/v1/history?limit=120");
                }
                final JSONArray items = r.optJSONArray("items");
                final boolean hasMore = r.optBoolean("has_more", false);
                ui.post(() -> {
                    if (!isActive(my) || target != listBox || tab != currentTab) return;
                    try {
                        if (tab == 0) {
                            renderSeries(items);
                            if (hasMore) addLoadMore();
                        } else if (tab == 1) {
                            target.removeAllViews();
                            renderVideos(items, false);
                            if (hasMore) addLoadMore();
                            if (target.getChildCount() == 0) addEmpty("还没有电影记录");
                        } else {
                            renderHistory(items, q);
                        }
                    } catch (Exception e) {
                        showError(friendly(e));
                    }
                });
            } catch (Exception e) {
                postError(my, e);
            }
        });
    }

    private void postError(int my, Exception e) {
        ui.post(() -> {
            if (isActive(my)) showError(friendly(e));
        });
    }

    private void showError(String msg) {
        if (listBox == null || !isUsable()) return;
        listBox.removeAllViews();
        LinearLayout box = card();
        box.setPadding(dp(16), dp(16), dp(16), dp(16));
        TextView title = text("读取失败", 14, C_RED, true);
        box.addView(title);
        TextView detail = text(msg, 12, C_TEXT2, false);
        detail.setLineSpacing(dp(3), 1f);
        box.addView(detail, marginTop(dp(6)));
        TextView retry = secondaryButton("重试");
        retry.setOnClickListener(v -> reload());
        box.addView(retry, marginTop(dp(13)));
        listBox.addView(box, match());
    }

    private View loadingRow() {
        LinearLayout box = column();
        box.setGravity(Gravity.CENTER);
        box.setPadding(0, dp(48), 0, dp(48));
        ProgressBar bar = new ProgressBar(this);
        bar.setIndeterminateTintList(ColorStateList.valueOf(C_ACCENT));
        box.addView(bar, new LinearLayout.LayoutParams(dp(28), dp(28)));
        TextView t = text("正在读取…", 12, C_TEXT3, false);
        t.setGravity(Gravity.CENTER);
        box.addView(t, marginTop(dp(10)));
        return box;
    }

    private void addEmpty(String msg) {
        LinearLayout box = column();
        box.setGravity(Gravity.CENTER);
        box.setPadding(dp(20), dp(54), dp(20), dp(54));
        TextView icon = text("◌", 30, C_TEXT3, false);
        icon.setGravity(Gravity.CENTER);
        box.addView(icon);
        TextView t = text(msg, 13, C_TEXT3, false);
        t.setGravity(Gravity.CENTER);
        box.addView(t, marginTop(dp(8)));
        listBox.addView(box, match());
    }

    // ============================================================
    // 列表渲染
    // ============================================================

    private void renderSeries(JSONArray items) {
        listBox.removeAllViews();
        if (items == null || items.length() == 0) {
            addEmpty(query.trim().isEmpty() ? "还没有剧集记录" : "没有匹配的剧集");
            return;
        }
        for (int i = 0; i < items.length(); i++) {
            JSONObject s = items.optJSONObject(i);
            if (s == null) continue;
            listBox.addView(seriesCard(s));
        }
    }

    private void appendSeries(JSONArray items) {
        if (items == null) return;
        for (int i = 0; i < items.length(); i++) {
            JSONObject s = items.optJSONObject(i);
            if (s != null) listBox.addView(seriesCard(s));
        }
    }

    private View seriesCard(JSONObject s) {
        LinearLayout card = card();
        card.setPadding(dp(15), dp(14), dp(15), dp(14));
        card.setOnClickListener(v -> openSeries(s.optLong("id"), s.optString("name", "剧集")));

        LinearLayout top = new LinearLayout(this);
        top.setOrientation(LinearLayout.HORIZONTAL);
        top.setGravity(Gravity.CENTER_VERTICAL);
        TextView name = text(s.optString("name", "未命名剧集"), 15, C_TEXT1, true);
        name.setMaxLines(2);
        top.addView(name, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        double rating = s.optDouble("rating", 0);
        if (rating > 0) {
            TextView rate = text(String.format(Locale.CHINA, "★ %.1f", rating), 11, C_ACCENT, true);
            LinearLayout.LayoutParams rlp = wrap();
            rlp.leftMargin = dp(8);
            top.addView(rate, rlp);
        }
        card.addView(top, match());

        int total = s.optInt("total_episodes", 0);
        int watched = s.optInt("watched_episodes", 0);
        double progress = s.optDouble("progress", 0);
        String meta = watched + " / " + total + " 集已看";
        String last = s.optString("last_watched_at", "");
        if (!last.isEmpty()) meta += " · 最近 " + shortDate(last);
        TextView sub = text(meta, 11, C_TEXT3, false);
        card.addView(sub, marginTop(dp(5)));

        ProgressBar bar = progressBar(progress);
        card.addView(bar, marginTop(dp(10)));

        LinearLayout bottom = new LinearLayout(this);
        bottom.setOrientation(LinearLayout.HORIZONTAL);
        bottom.setGravity(Gravity.CENTER_VERTICAL);
        TextView pct = text(Math.round(progress) + "%", 10, progress >= 99.9 ? C_GREEN : C_TEXT3, true);
        bottom.addView(pct);
        TextView tags = text(tagText(s.optJSONArray("tags")), 10, C_TEXT3, false);
        tags.setGravity(Gravity.END);
        bottom.addView(tags, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        TextView open = text("查看剧集 ›", 11, C_ACCENT, true);
        bottom.addView(open);
        card.addView(bottom, marginTop(dp(4)));

        LinearLayout.LayoutParams lp = match();
        lp.bottomMargin = dp(10);
        card.setLayoutParams(lp);
        return card;
    }

    private void renderVideos(JSONArray items, boolean history) throws Exception {
        if (items == null) return;
        for (int i = 0; i < items.length(); i++) {
            JSONObject v = items.optJSONObject(i);
            if (v != null) listBox.addView(videoCard(v, history));
        }
    }

    private View videoCard(JSONObject v, boolean history) {
        boolean available = v.optBoolean("available", true);
        LinearLayout card = card();
        card.setPadding(dp(15), dp(13), dp(15), dp(13));

        LinearLayout top = new LinearLayout(this);
        top.setOrientation(LinearLayout.HORIZONTAL);
        top.setGravity(Gravity.CENTER_VERTICAL);
        TextView title = text(v.optString("title", "未命名"), 14, available ? C_TEXT1 : C_TEXT3, true);
        title.setMaxLines(2);
        top.addView(title, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));

        TextView play = text(available ? "▶ VLC" : "离线", 10,
                available ? C_ACCENT_INK : C_TEXT3, true);
        play.setGravity(Gravity.CENTER);
        play.setPadding(dp(10), dp(5), dp(10), dp(5));
        play.setBackground(roundRect(available ? C_ACCENT : C_BG3, dp(6), 0, 0));
        LinearLayout.LayoutParams plp = wrap();
        plp.leftMargin = dp(9);
        top.addView(play, plp);
        card.addView(top, match());

        StringBuilder meta = new StringBuilder();
        String series = v.optString("series_name", "");
        if (!series.isEmpty()) {
            meta.append(series);
            int season = v.optInt("season", 0);
            int episode = v.optInt("episode", 0);
            if (episode > 0) {
                meta.append(" · ");
                if (season > 0) meta.append("S").append(String.format(Locale.CHINA, "%02d", season));
                meta.append("E").append(String.format(Locale.CHINA, "%02d", episode));
            }
            meta.append(" · ");
        }
        double duration = v.optDouble("duration", 0);
        if (duration > 0) meta.append(durationText(duration)).append(" · ");
        meta.append(formatSize(v.optLong("file_size", 0)));
        TextView sub = text(meta.toString(), 10.5f, C_TEXT3, false);
        card.addView(sub, marginTop(dp(4)));

        double progress = v.optDouble("watch_progress", 0);
        if (progress > 0.1 || history) {
            String line = "观看进度 " + Math.round(progress) + "%";
            if (history) {
                String at = v.optString("watched_at", "");
                if (!at.isEmpty()) line += " · " + shortDate(at);
            }
            TextView p = text(line, 10, progress >= 99.9 ? C_GREEN : C_TEXT2, false);
            card.addView(p, marginTop(dp(7)));
            if (progress > 0.1) {
                card.addView(progressBar(progress), marginTop(dp(6)));
            }
        }

        card.setOnClickListener(x -> showVideoDialog(v));
        play.setOnClickListener(x -> playVideo(v));
        LinearLayout.LayoutParams lp = match();
        lp.bottomMargin = dp(9);
        card.setLayoutParams(lp);
        return card;
    }

    private void renderHistory(JSONArray items, String filter) {
        listBox.removeAllViews();
        if (items == null || items.length() == 0) {
            addEmpty("还没有播放记录");
            return;
        }
        String f = filter == null ? "" : filter.toLowerCase(Locale.ROOT);
        int shown = 0;
        for (int i = 0; i < items.length(); i++) {
            JSONObject entry = items.optJSONObject(i);
            if (entry == null) continue;
            JSONObject v = entry.optJSONObject("video");
            if (v == null) continue;
            if (!f.isEmpty()) {
                String hay = (v.optString("title", "") + " "
                        + v.optString("series_name", "")).toLowerCase(Locale.ROOT);
                if (!hay.contains(f)) continue;
            }
            try {
                // 历史条目把 watched_at 提到视频对象上，复用同一种卡片。
                v.put("watched_at", entry.optString("watched_at", ""));
                if (entry.has("progress")) v.put("watch_progress", entry.optDouble("progress", 0));
            } catch (Exception ignored) {
            }
            listBox.addView(videoCard(v, true));
            shown++;
        }
        if (shown == 0) addEmpty("没有匹配的历史记录");
    }

    private void addLoadMore() {
        final int tab = currentTab;
        final int my = generation;
        TextView more = secondaryButton("加载更多");
        more.setOnClickListener(v -> {
            if (!isActive(my) || tab != currentTab || listBox == null) return;
            int page;
            if (tab == 0) {
                page = ++seriesPage;
            } else if (tab == 1) {
                page = ++moviePage;
            } else {
                return;
            }
            listBox.removeView(more);
            reloadPageAppend(tab, page, my);
        });
        listBox.addView(more, marginTop(dp(4)));
    }

    private void reloadPageAppend(int tab, int page, int my) {
        final LinearLayout target = listBox;
        final String q = query.trim();
        cancelListRequest();
        final ApiClient client = new ApiClient(baseUrl, token);
        listClient = client;
        final View loading = loadingRow();
        target.addView(loading);
        executeIo(() -> {
            if (!isActive(my)) return;
            try {
                String path = tab == 0
                        ? "/api/v1/series?sort=recent&limit=40&page=" + page
                        : "/api/v1/videos?kind=movie&limit=40&page=" + page;
                if (!q.isEmpty()) path += "&q=" + ApiClient.urlEncode(q);
                JSONObject r = client.get(path);
                JSONArray items = r.optJSONArray("items");
                boolean more = r.optBoolean("has_more", false);
                ui.post(() -> {
                    if (!isActive(my) || target != listBox || tab != currentTab) return;
                    target.removeView(loading);
                    try {
                        if (tab == 0) {
                            appendSeries(items);
                        } else {
                            renderVideos(items, false);
                        }
                        if (more) addLoadMore();
                    } catch (Exception e) {
                        handleAppendError(tab, page, my, target, friendly(e));
                    }
                });
            } catch (Exception e) {
                ui.post(() -> {
                    if (!isActive(my) || target != listBox || tab != currentTab) return;
                    target.removeView(loading);
                    handleAppendError(tab, page, my, target, friendly(e));
                });
            }
        });
    }

    private void handleAppendError(int tab, int page, int my, LinearLayout target, String msg) {
        if (!isActive(my) || target != listBox || tab != currentTab) return;
        if (tab == 0 && seriesPage == page) {
            seriesPage--;
        } else if (tab == 1 && moviePage == page) {
            moviePage--;
        }
        TextView error = text("加载更多失败：" + msg + "\n已保留当前内容，可点击加载更多重试。",
                12, C_RED, false);
        error.setLineSpacing(dp(3), 1f);
        target.addView(error, marginTop(dp(10)));
        addLoadMore();
    }

    // ============================================================
    // 剧集详情
    // ============================================================

    private void openSeries(long id, String name) {
        cancelPendingSearch();
        cancelAllRequests();
        inDetail = true;
        detailId = id;
        detailName = name;
        generation++;
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(C_BG0);
        LinearLayout page = column();
        page.setPadding(sidePad, 0, sidePad, 0);
        root.addView(page, match());

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(0, dp(15), 0, dp(12));
        TextView back = actionChip("‹ 影库");
        back.setOnClickListener(v -> showLibrary());
        row.addView(back);
        TextView title = text(name, 17, C_TEXT1, true);
        title.setGravity(Gravity.END);
        title.setMaxLines(1);
        title.setEllipsize(android.text.TextUtils.TruncateAt.END);
        LinearLayout.LayoutParams tlp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        tlp.leftMargin = dp(12);
        row.addView(title, tlp);
        page.addView(row, wrap());

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        page.addView(scroll, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));
        LinearLayout box = column();
        box.setPadding(0, dp(2), 0, dp(30));
        scroll.addView(box, new ScrollView.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
        ));
        box.addView(loadingRow());

        setContentView(root);
        final int my = generation;
        final ApiClient client = new ApiClient(baseUrl, token);
        detailClient = client;
        executeIo(() -> {
            if (!isActive(my)) return;
            try {
                JSONObject r = client.get("/api/v1/series/" + id);
                ui.post(() -> {
                    if (!isActive(my)) return;
                    box.removeAllViews();
                    try {
                        renderSeriesDetail(box, r);
                    } catch (Exception e) {
                        TextView t = text(friendly(e), 12, C_RED, false);
                        box.addView(t);
                    }
                });
            } catch (Exception e) {
                ui.post(() -> {
                    if (!isActive(my)) return;
                    box.removeAllViews();
                    TextView t = text(friendly(e), 12, C_RED, false);
                    t.setLineSpacing(dp(3), 1f);
                    box.addView(t);
                    TextView retry = secondaryButton("重试");
                    retry.setOnClickListener(v -> openSeries(id, name));
                    box.addView(retry, marginTop(dp(12)));
                });
            }
        });
    }

    private void renderSeriesDetail(LinearLayout box, JSONObject r) throws Exception {
        JSONObject s = r.optJSONObject("series");
        JSONArray eps = r.optJSONArray("episodes");
        if (s == null) throw new Exception("剧集数据不完整");

        LinearLayout hero = card();
        hero.setPadding(dp(16), dp(15), dp(16), dp(15));
        TextView title = text(s.optString("name", ""), 20, C_TEXT1, true);
        hero.addView(title, match());
        int total = s.optInt("total_episodes", 0);
        int watched = s.optInt("watched_episodes", 0);
        double progress = s.optDouble("progress", 0);
        TextView meta = text(watched + " / " + total + " 集已看 · " + Math.round(progress) + "%",
                12, progress >= 99.9 ? C_GREEN : C_TEXT2, false);
        hero.addView(meta, marginTop(dp(6)));
        hero.addView(progressBar(progress), marginTop(dp(10)));
        double rating = s.optDouble("rating", 0);
        String tags = tagText(s.optJSONArray("tags"));
        if (rating > 0 || !tags.isEmpty()) {
            String x = rating > 0 ? String.format(Locale.CHINA, "★ %.1f", rating) : "";
            if (!x.isEmpty() && !tags.isEmpty()) x += "  ·  ";
            x += tags;
            hero.addView(text(x, 10.5f, C_TEXT3, false), marginTop(dp(9)));
        }
        LinearLayout.LayoutParams hlp = match();
        hlp.bottomMargin = dp(16);
        hero.setLayoutParams(hlp);
        box.addView(hero);

        if (eps == null || eps.length() == 0) {
            addInto(box, "这部剧集没有单集记录");
            return;
        }
        TextView section = text("单集 · " + eps.length(), 11, C_TEXT3, true);
        section.setLetterSpacing(0.08f);
        box.addView(section, marginBottom(dp(8)));

        for (int i = 0; i < eps.length(); i++) {
            JSONObject v = eps.optJSONObject(i);
            if (v != null) box.addView(episodeCard(v, i + 1));
        }
    }

    private void addInto(LinearLayout box, String msg) {
        TextView t = text(msg, 13, C_TEXT3, false);
        t.setGravity(Gravity.CENTER);
        t.setPadding(0, dp(40), 0, dp(40));
        box.addView(t, match());
    }

    private View episodeCard(JSONObject v, int fallbackIndex) {
        boolean available = v.optBoolean("available", true);
        LinearLayout card = card();
        card.setPadding(dp(14), dp(12), dp(14), dp(12));

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);

        int season = v.optInt("season", 0);
        int episode = v.optInt("episode", 0);
        String label;
        if (episode > 0) {
            label = (season > 0 ? "S" + String.format(Locale.CHINA, "%02d", season) : "")
                    + "E" + String.format(Locale.CHINA, "%02d", episode);
        } else {
            label = String.format(Locale.CHINA, "%02d", fallbackIndex);
        }
        TextView ep = text(label, 11, available ? C_ACCENT : C_TEXT3, true);
        ep.setGravity(Gravity.CENTER);
        ep.setPadding(dp(7), dp(5), dp(7), dp(5));
        ep.setBackground(roundRect(available ? 0x22F0B429 : C_BG3, dp(5), 0, 0));
        row.addView(ep, wrap());

        LinearLayout mid = column();
        LinearLayout.LayoutParams mlp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        mlp.leftMargin = dp(10);
        TextView title = text(v.optString("title", "未命名"), 13, available ? C_TEXT1 : C_TEXT3, true);
        title.setMaxLines(1);
        title.setEllipsize(android.text.TextUtils.TruncateAt.END);
        mid.addView(title, match());
        StringBuilder sub = new StringBuilder();
        double dur = v.optDouble("duration", 0);
        if (dur > 0) sub.append(durationText(dur));
        long size = v.optLong("file_size", 0);
        if (size > 0) {
            if (sub.length() > 0) sub.append(" · ");
            sub.append(formatSize(size));
        }
        double p = v.optDouble("watch_progress", 0);
        if (p > 0.1) {
            if (sub.length() > 0) sub.append(" · ");
            sub.append(p >= 99.9 ? "已看完" : "看至 " + Math.round(p) + "%");
        }
        if (!available) {
            if (sub.length() > 0) sub.append(" · ");
            sub.append("文件暂不可用");
        }
        TextView detail = text(sub.toString(), 10, C_TEXT3, false);
        mid.addView(detail, marginTop(dp(3)));
        if (p > 0.1) mid.addView(progressBar(p), marginTop(dp(5)));
        row.addView(mid, mlp);

        TextView play = text(available ? "▶" : "—", 15,
                available ? C_ACCENT : C_TEXT3, true);
        play.setGravity(Gravity.CENTER);
        play.setPadding(dp(10), dp(7), dp(6), dp(7));
        row.addView(play, wrap());

        card.addView(row, match());
        card.setOnClickListener(x -> {
            if (available) playVideo(v);
            else toast("这一集的文件当前不可用");
        });
        LinearLayout.LayoutParams lp = match();
        lp.bottomMargin = dp(8);
        card.setLayoutParams(lp);
        return card;
    }

    // ============================================================
    // 播放与视频详情
    // ============================================================

    private void showVideoDialog(JSONObject v) {
        if (v == null) {
            toast("视频信息不可用");
            return;
        }
        if (!isUsable()) return;
        boolean available = v.optBoolean("available", true);
        StringBuilder msg = new StringBuilder();
        String series = v.optString("series_name", "");
        if (!series.isEmpty()) msg.append(series);
        int season = v.optInt("season", 0);
        int episode = v.optInt("episode", 0);
        if (episode > 0) {
            if (msg.length() > 0) msg.append(" · ");
            if (season > 0) msg.append("S").append(String.format(Locale.CHINA, "%02d", season));
            msg.append("E").append(String.format(Locale.CHINA, "%02d", episode));
        }
        if (msg.length() > 0) msg.append("\n");
        msg.append("片长 ").append(durationText(v.optDouble("duration", 0)));
        msg.append(" · ").append(formatSize(v.optLong("file_size", 0)));
        double p = v.optDouble("watch_progress", 0);
        msg.append("\n观看进度 ").append(Math.round(p)).append("%");
        msg.append(available ? "\n\n将使用 VLC 直接读取电脑上的原始文件。" : "\n\n文件当前不可用，请检查外接硬盘是否连接。");

        AlertDialog.Builder b = new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
                .setTitle(v.optString("title", "视频"))
                .setMessage(msg.toString())
                .setNegativeButton("关闭", null);
        if (available) {
            b.setPositiveButton("用 VLC 播放", (d, w) -> playVideo(v));
        }
        AlertDialog d = b.show();
        d.setOnShowListener(x -> {
            if (d.getButton(AlertDialog.BUTTON_POSITIVE) != null) {
                d.getButton(AlertDialog.BUTTON_POSITIVE).setTextColor(C_ACCENT);
            }
            if (d.getButton(AlertDialog.BUTTON_NEGATIVE) != null) {
                d.getButton(AlertDialog.BUTTON_NEGATIVE).setTextColor(C_TEXT2);
            }
        });
    }

    private void playVideo(JSONObject v) {
        if (v == null) {
            toast("视频信息不可用");
            return;
        }
        if (!v.optBoolean("available", true)) {
            toast("文件当前不可用（外接硬盘未连接或文件已移动）");
            return;
        }
        final long id = v.optLong("id", 0);
        if (id <= 0) {
            toast("视频 id 无效");
            return;
        }
        if (!isUsable()) return;
        if (playPending) {
            toast("正在获取播放凭证，请稍候");
            return;
        }
        playPending = true;
        final int my = generation;
        final int connection = connectionGeneration;
        cancelMediaRequest();
        final ApiClient client = new ApiClient(baseUrl, token);
        mediaClient = client;
        toast("正在获取 6 小时播放凭证…");
        executeIo(() -> {
            if (!isConnectionActive(my, connection)) return;
            try {
                String ticket = client.mediaTicket(id);
                String url = client.mediaUrl(id, ticket);
                ui.post(() -> {
                    playPending = false;
                    if (!isConnectionActive(my, connection)) return;
                    launchVlc(v, url);
                });
            } catch (Exception e) {
                ui.post(() -> {
                    playPending = false;
                    if (isConnectionActive(my, connection)) {
                        toast("获取播放凭证失败：" + friendly(e));
                    }
                });
            }
        });
    }

    private void launchVlc(JSONObject v, String url) {
        if (!isUsable()) return;
        Intent view = new Intent(Intent.ACTION_VIEW);
        view.setDataAndType(Uri.parse(url), "video/*");
        view.putExtra(Intent.EXTRA_TITLE, v.optString("title", "CineVault"));
        view.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);

        Intent vlc = new Intent(view);
        vlc.setPackage("org.videolan.vlc");
        try {
            startActivity(vlc);
            return;
        } catch (ActivityNotFoundException | SecurityException ignored) {
        }

        try {
            startActivity(Intent.createChooser(view, "选择播放器"));
        } catch (ActivityNotFoundException e) {
            new AlertDialog.Builder(this)
                    .setTitle("没有可用的播放器")
                    .setMessage("建议安装 VLC for Android。安装后回到本页再试，即可一键播放。")
                    .setPositiveButton("去安装 VLC", (d, w) -> {
                        try {
                            startActivity(new Intent(Intent.ACTION_VIEW,
                                    Uri.parse("market://details?id=org.videolan.vlc")));
                        } catch (Exception ex) {
                            startActivity(new Intent(Intent.ACTION_VIEW,
                                    Uri.parse("https://www.videolan.org/vlc/download-android.html")));
                        }
                    })
                    .setNegativeButton("取消", null)
                    .show();
        }
    }

    private boolean isUsable() {
        return !destroyed && !isFinishing() && !isDestroyed();
    }

    private boolean isActive(int expectedGeneration) {
        return isUsable() && generation == expectedGeneration;
    }

    private boolean isConnectionActive(int expectedGeneration, int expectedConnection) {
        return isActive(expectedGeneration) && connectionGeneration == expectedConnection;
    }

    private void cancelPendingSearch() {
        if (searchRunnable != null) {
            ui.removeCallbacks(searchRunnable);
            searchRunnable = null;
        }
    }

    private void cancelListRequest() {
        ApiClient c = listClient;
        listClient = null;
        if (c != null) c.cancel();
    }

    private void cancelOverviewRequest() {
        ApiClient c = overviewClient;
        overviewClient = null;
        if (c != null) c.cancel();
    }

    private void cancelConnectRequest() {
        ApiClient c = connectClient;
        connectClient = null;
        if (c != null) c.cancel();
    }

    private void cancelMediaRequest() {
        ApiClient c = mediaClient;
        mediaClient = null;
        if (c != null) c.cancel();
    }

    private void cancelAllRequests() {
        ApiClient c;
        c = listClient; listClient = null; if (c != null) c.cancel();
        c = overviewClient; overviewClient = null; if (c != null) c.cancel();
        c = detailClient; detailClient = null; if (c != null) c.cancel();
        c = connectClient; connectClient = null; if (c != null) c.cancel();
        c = mediaClient; mediaClient = null; if (c != null) c.cancel();
    }

    private void executeIo(Runnable task) {
        if (destroyed || io == null) return;
        try {
            io.execute(task);
        } catch (java.util.concurrent.RejectedExecutionException ignored) {
            // Activity 销毁后不再接受新请求。
        }
    }

    // ============================================================
    // 基础组件与工具
    // ============================================================

    private View brandBlock(boolean compact) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);

        TextView mark = text("CV", 19, C_ACCENT_INK, true);
        mark.setGravity(Gravity.CENTER);
        mark.setBackground(roundRect(C_ACCENT, dp(12), 0, 0));
        row.addView(mark, new LinearLayout.LayoutParams(dp(46), dp(46)));

        LinearLayout mid = column();
        LinearLayout.LayoutParams mlp = new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f);
        mlp.leftMargin = dp(12);
        TextView name = text("CineVault", compact ? 16 : 19, C_TEXT1, true);
        mid.addView(name);
        TextView sub = text("MOBILE  ·  LAN READ-ONLY", 9, C_TEXT3, true);
        sub.setLetterSpacing(0.12f);
        mid.addView(sub, marginTop(dp(2)));
        row.addView(mid, mlp);
        return row;
    }

    private TextView fieldLabel(String s) {
        TextView t = text(s, 11, C_TEXT3, true);
        t.setLetterSpacing(0.04f);
        return t;
    }

    private EditText input(String hint) {
        EditText e = new EditText(this);
        e.setHint(hint);
        e.setHintTextColor(C_TEXT3);
        e.setTextColor(C_TEXT1);
        e.setTextSize(TypedValue.COMPLEX_UNIT_SP, 13.5f);
        e.setSingleLine(true);
        e.setPadding(dp(13), dp(11), dp(13), dp(11));
        e.setBackground(roundRect(C_BG1, dp(9), 0x33FFFFFF, 1));
        e.setSelectAllOnFocus(false);
        return e;
    }

    private TextView primaryButton(String label) {
        TextView b = text(label, 13.5f, C_ACCENT_INK, true);
        b.setGravity(Gravity.CENTER);
        b.setPadding(dp(12), dp(13), dp(12), dp(13));
        b.setBackground(roundRect(C_ACCENT, dp(9), 0, 0));
        b.setClickable(true);
        return b;
    }

    private TextView secondaryButton(String label) {
        TextView b = text(label, 12.5f, C_TEXT2, true);
        b.setGravity(Gravity.CENTER);
        b.setPadding(dp(12), dp(11), dp(12), dp(11));
        b.setBackground(roundRect(C_BG2, dp(9), 0x33FFFFFF, 1));
        b.setClickable(true);
        return b;
    }

    private TextView ghostButton(String label) {
        TextView b = text(label, 11.5f, C_TEXT2, true);
        b.setGravity(Gravity.CENTER);
        b.setPadding(dp(10), dp(9), dp(10), dp(9));
        b.setBackground(roundRect(C_BG1, dp(8), 0x22FFFFFF, 1));
        b.setClickable(true);
        return b;
    }

    private TextView actionChip(String label) {
        TextView b = text(label, 11.5f, C_TEXT2, true);
        b.setGravity(Gravity.CENTER);
        b.setPadding(dp(11), dp(7), dp(11), dp(7));
        b.setBackground(roundRect(C_BG1, dp(7), 0x2AFFFFFF, 1));
        b.setClickable(true);
        return b;
    }

    private void infoNote(LinearLayout page, String title, String body) {
        LinearLayout box = card();
        box.setPadding(dp(15), dp(13), dp(15), dp(13));
        TextView t = text(title, 10.5f, C_ACCENT, true);
        t.setLetterSpacing(0.08f);
        box.addView(t);
        TextView b = text(body, 11.5f, C_TEXT3, false);
        b.setLineSpacing(dp(3), 1f);
        box.addView(b, marginTop(dp(5)));
        page.addView(box, match());
    }

    private ProgressBar progressBar(double percent) {
        ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        bar.setMax(1000);
        int p = (int) Math.round(Math.max(0, Math.min(100, percent)) * 10);
        bar.setProgress(p);
        bar.setProgressTintList(ColorStateList.valueOf(percent >= 99.9 ? C_GREEN : C_ACCENT));
        bar.setProgressBackgroundTintList(ColorStateList.valueOf(C_BG3));
        bar.setMinimumHeight(dp(3));
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, dp(3)
        );
        bar.setLayoutParams(lp);
        return bar;
    }

    private LinearLayout card() {
        LinearLayout c = column();
        c.setBackground(roundRect(C_BG1, dp(12), 0x1FFFFFFF, 1));
        c.setClickable(true);
        return c;
    }

    private GradientDrawable roundRect(int fill, float radius, int stroke, int strokeWidth) {
        GradientDrawable d = new GradientDrawable();
        d.setColor(fill);
        d.setCornerRadius(radius);
        if (strokeWidth > 0) d.setStroke(strokeWidth, stroke);
        return d;
    }

    private LinearLayout column() {
        LinearLayout l = new LinearLayout(this);
        l.setOrientation(LinearLayout.VERTICAL);
        return l;
    }

    private TextView text(String s, float size, int color, boolean bold) {
        TextView t = new TextView(this);
        t.setText(s);
        t.setTextSize(TypedValue.COMPLEX_UNIT_SP, size);
        t.setTextColor(color);
        if (bold) t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        return t;
    }

    private LinearLayout.LayoutParams match() {
        return new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
        );
    }

    private LinearLayout.LayoutParams wrap() {
        return new LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
        );
    }

    private LinearLayout.LayoutParams marginTop(int px) {
        LinearLayout.LayoutParams lp = match();
        lp.topMargin = px;
        return lp;
    }

    private LinearLayout.LayoutParams marginBottom(int px) {
        LinearLayout.LayoutParams lp = match();
        lp.bottomMargin = px;
        return lp;
    }

    private void space(LinearLayout parent, int px) {
        View v = new View(this);
        parent.addView(v, new LinearLayout.LayoutParams(1, px));
    }

    private int dp(float n) {
        return Math.round(n * density);
    }

    private String friendly(Exception e) {
        String m = e.getMessage();
        if (m == null || m.trim().isEmpty()) {
            m = e.getClass().getSimpleName();
        }
        if (m.contains("Failed to connect") || m.contains("Connection refused")
                || m.contains("ECONNREFUSED")) {
            return "连接不到电脑。请确认 CineVault 正在运行、「手机观看」已开启，且手机与电脑在同一 Wi-Fi。";
        }
        if (m.contains("timeout") || m.contains("timed out")) {
            return "连接超时。请检查电脑防火墙是否允许 CineVault 的专用网络访问。";
        }
        return m;
    }

    private String tagText(JSONArray tags) {
        if (tags == null || tags.length() == 0) return "";
        StringBuilder b = new StringBuilder();
        for (int i = 0; i < tags.length() && i < 3; i++) {
            JSONObject t = tags.optJSONObject(i);
            if (t == null) continue;
            if (b.length() > 0) b.append(" · ");
            b.append(t.optString("name", ""));
        }
        return b.toString();
    }

    private String durationText(double seconds) {
        if (seconds <= 0) return "片长未知";
        long s = Math.round(seconds);
        long h = s / 3600;
        long m = (s % 3600) / 60;
        if (h > 0) return h + " 小时 " + m + " 分";
        return Math.max(1, m) + " 分钟";
    }

    private String formatSize(long bytes) {
        if (bytes <= 0) return "0 B";
        double kb = bytes / 1024.0;
        if (kb < 1024) return String.format(Locale.CHINA, "%.0f KB", kb);
        double mb = kb / 1024.0;
        if (mb < 1024) return String.format(Locale.CHINA, "%.0f MB", mb);
        double gb = mb / 1024.0;
        if (gb < 1024) return String.format(Locale.CHINA, "%.1f GB", gb);
        return String.format(Locale.CHINA, "%.2f TB", gb / 1024.0);
    }

    private String shortDate(String date) {
        if (date == null || date.length() < 10) return date == null ? "" : date;
        String d = date.substring(0, 10);
        String t = date.length() >= 16 ? date.substring(11, 16) : "";
        return d + (t.isEmpty() ? "" : " " + t);
    }

    private void toast(String msg) {
        Toast.makeText(this, msg, Toast.LENGTH_SHORT).show();
    }
}
