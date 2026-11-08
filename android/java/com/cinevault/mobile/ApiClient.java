package com.cinevault.mobile;

import org.json.JSONException;
import org.json.JSONObject;

import java.io.BufferedInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.URL;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;

/**
 * CineVault 局域网 API 的极简客户端。
 * 只发 GET：手机端是严格只读的，连 POST 入口都不提供。
 */
final class ApiClient {
    static final int DEFAULT_PORT = 42337;
    private static final int MAX_BODY_BYTES = 4 * 1024 * 1024;

    private final String baseUrl;
    private final String token;
    private volatile HttpURLConnection activeConnection;
    private volatile boolean cancelled;

    ApiClient(String baseUrl, String token) {
        this.baseUrl = baseUrl;
        this.token = token;
    }

    JSONObject get(String path) throws IOException, JSONException {
        return request(baseUrl + path, token, 5000, 20000, null, null, this);
    }

    /** 页面代际已失效时主动断开，避免过期搜索继续占用下载带宽和线程。 */
    void cancel() {
        cancelled = true;
        HttpURLConnection conn = activeConnection;
        if (conn != null) {
            try {
                conn.disconnect();
            } catch (Exception ignored) {
            }
        }
    }

    JSONObject pairWith(String code) throws IOException, JSONException {
        return request(baseUrl + "/api/v1/pair", null, 5000, 12000,
                "X-CineVault-Pair", code, this);
    }

    String mediaTicket(long videoId) throws IOException, JSONException {
        if (videoId <= 0) throw new IOException("视频 id 无效");
        JSONObject response = get("/api/v1/media-ticket?id=" + videoId);
        String ticket = response.optString("ticket", "");
        if (ticket.isEmpty()) throw new IOException("服务器未返回有效播放凭证");
        return ticket;
    }

    String mediaUrl(long videoId, String ticket) {
        return baseUrl + "/api/v1/media/" + videoId + "?ticket=" + urlEncode(ticket);
    }

    private static JSONObject request(String url, String token, int connectMs, int readMs,
                                      String extraHeader, String extraValue, ApiClient owner)
            throws IOException, JSONException {
        HttpURLConnection conn = (HttpURLConnection) new URL(url).openConnection();
        if (owner != null) {
            owner.activeConnection = conn;
            if (owner.cancelled) {
                conn.disconnect();
                if (owner.activeConnection == conn) owner.activeConnection = null;
                throw new IOException("请求已取消");
            }
        }
        conn.setRequestMethod("GET");
        conn.setConnectTimeout(connectMs);
        conn.setReadTimeout(readMs);
        conn.setRequestProperty("Accept", "application/json");
        conn.setRequestProperty("User-Agent", "CineVault-Android/1.0");
        if (token != null && !token.isEmpty()) {
            conn.setRequestProperty("Authorization", "Bearer " + token);
        }
        if (extraHeader != null && extraValue != null) {
            conn.setRequestProperty(extraHeader, extraValue);
        }
        try {
            int code = conn.getResponseCode();
            InputStream raw = code >= 200 && code < 300
                    ? conn.getInputStream()
                    : conn.getErrorStream();
            String body = readAll(raw);
            if (code < 200 || code >= 300) {
                String message = "服务器返回 HTTP " + code;
                boolean serverMessage = false;
                try {
                    JSONObject err = new JSONObject(body);
                    if (err.has("error")) {
                        message = err.getString("error");
                        serverMessage = true;
                    }
                } catch (Exception ignored) {
                }
                if (code == 401) message = "访问令牌无效，请重新配对";
                if (code == 403 && !serverMessage) message = "配对码不正确";
                throw new IOException(message);
            }
            return new JSONObject(body);
        } finally {
            if (owner != null && owner.activeConnection == conn) {
                owner.activeConnection = null;
            }
            conn.disconnect();
        }
    }

    private static String readAll(InputStream in) throws IOException {
        if (in == null) return "";
        try (InputStream input = new BufferedInputStream(in);
             ByteArrayOutputStream out = new ByteArrayOutputStream()) {
            byte[] buf = new byte[16 * 1024];
            int n;
            while ((n = input.read(buf)) >= 0) {
                if (out.size() + n > MAX_BODY_BYTES) {
                    throw new IOException("服务器响应超过 4 MiB 限制");
                }
                out.write(buf, 0, n);
            }
            return out.toString(StandardCharsets.UTF_8.name());
        }
    }

    static String normalizeBase(String raw) throws IOException {
        String s = raw == null ? "" : raw.trim();
        if (s.isEmpty()) throw new IOException("请填写电脑地址");
        if (!s.contains("://")) s = "http://" + s;
        try {
            URI u = new URI(s);
            if (!"http".equalsIgnoreCase(u.getScheme())) {
                throw new IOException("桌面端未启用 TLS，地址仅支持 http://");
            }
            if (u.getHost() == null || u.getHost().isEmpty()
                    || u.getRawUserInfo() != null || u.getRawQuery() != null
                    || u.getRawFragment() != null) {
                throw new IOException("电脑地址格式不正确");
            }
            String path = u.getRawPath();
            if (path != null && !path.isEmpty() && !"/".equals(path)) {
                throw new IOException("地址只填电脑 IP 和端口，不要带路径、查询或片段");
            }
            int port = u.getPort();
            if (port == 0 || port > 65535) throw new IOException("电脑端口无效");
            if (port < 0) port = DEFAULT_PORT;
            String host = u.getHost();
            if (host.contains(":") && !host.startsWith("[")) host = "[" + host + "]";
            return "http://" + host + ":" + port;
        } catch (URISyntaxException | IllegalArgumentException e) {
            throw new IOException("电脑地址格式不正确，例如 192.168.1.10:42337 或 [fe80::1]:42337");
        }
    }

    static String urlEncode(String s) {
        try {
            return URLEncoder.encode(s == null ? "" : s, StandardCharsets.UTF_8.name());
        } catch (Exception e) {
            return "";
        }
    }
}
