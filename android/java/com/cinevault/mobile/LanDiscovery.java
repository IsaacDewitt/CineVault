package com.cinevault.mobile;

import org.json.JSONObject;

import java.io.IOException;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.MessageDigest;
import java.security.SecureRandom;

import javax.crypto.Mac;
import javax.crypto.spec.SecretKeySpec;

/**
 * 局域网广播发现。响应必须用当前配对码证明其持有同一密钥。
 */
final class LanDiscovery {
    private static final int DISCOVERY_PORT = 42338;
    private static final String MAGIC = "CINEVAULT_DISCOVER_V2:";
    private static final char[] HEX = "0123456789abcdef".toCharArray();

    static String discover(String pairingCode) throws IOException {
        byte[] nonce = new byte[32];
        new SecureRandom().nextBytes(nonce);
        String nonceHex = hex(nonce);
        byte[] payload = (MAGIC + nonceHex).getBytes(StandardCharsets.US_ASCII);
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.setBroadcast(true);
            socket.setSoTimeout(1800);
            DatagramPacket out = new DatagramPacket(
                    payload,
                    payload.length,
                    InetAddress.getByName("255.255.255.255"),
                    DISCOVERY_PORT
            );
            socket.send(out);

            String expected = hmac(pairingCode, nonceHex);
            long deadline = System.currentTimeMillis() + 1800;
            while (System.currentTimeMillis() < deadline) {
                byte[] buf = new byte[2048];
                DatagramPacket in = new DatagramPacket(buf, buf.length);
                try {
                    socket.receive(in);
                } catch (java.net.SocketTimeoutException e) {
                    break;
                }
                try {
                    String body = new String(in.getData(), in.getOffset(), in.getLength(),
                            StandardCharsets.UTF_8);
                    JSONObject json = new JSONObject(body);
                    if (!"CineVault".equals(json.optString("app"))) continue;
                    int port = json.optInt("port", -1);
                    if (port <= 0 || port > 65535) continue;
                    String proof = json.optString("proof");
                    if (!proof.matches("[0-9a-f]{64}")) continue;
                    // 不信任“第一个响应”：只接受能证明持有当前配对码的服务器。
                    if (!MessageDigest.isEqual(proof.getBytes(StandardCharsets.US_ASCII),
                            expected.getBytes(StandardCharsets.US_ASCII))) {
                        continue;
                    }
                    String host = in.getAddress().getHostAddress();
                    if (host.contains(":") && !host.startsWith("[")) host = "[" + host + "]";
                    return "http://" + host + ":" + port;
                } catch (org.json.JSONException ignored) {
                    // 伪造/损坏响应被忽略，继续等待真正的电脑。
                }
            }
            throw new IOException("没有发现通过配对码验证的电脑。请确认配对码正确、"
                    + "CineVault 已开启「手机观看」，且手机与电脑在同一 Wi-Fi");
        }
    }

    private static String hmac(String key, String message) throws IOException {
        try {
            Mac mac = Mac.getInstance("HmacSHA256");
            mac.init(new SecretKeySpec(key.getBytes(StandardCharsets.UTF_8), "HmacSHA256"));
            return hex(mac.doFinal(message.getBytes(StandardCharsets.US_ASCII)));
        } catch (GeneralSecurityException e) {
            throw new IOException("无法校验发现响应", e);
        }
    }

    private static String hex(byte[] bytes) {
        char[] out = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            int value = bytes[i] & 0xff;
            out[i * 2] = HEX[value >>> 4];
            out[i * 2 + 1] = HEX[value & 0x0f];
        }
        return new String(out);
    }
}
