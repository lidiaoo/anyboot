package com.anyboot.remote;

import org.json.JSONException;
import org.json.JSONObject;

import java.util.UUID;

/**
 * 一把 SSH 密钥（密钥页的密钥卡）。
 *
 * <p>私钥 PEM（OpenSSH / PKCS8 文本）经 {@link KeyVault} 加密后存
 * {@link #privateEnc}；指纹在导入 / 生成时算好存 {@link #fingerprint}，
 * 列表页不用重新算。
 */
final class SshKeyEntry {

    String id;
    String name = "";
    /** 算法名（RSA / EC / Ed25519…），导入或生成时记录 */
    String algorithm = "";
    /** SHA256:base64 公钥指纹 */
    String fingerprint = "";
    /** 私钥 PEM 文本（KeyVault 加密落盘） */
    String privateEnc = "";
    /** 公钥（OpenSSH 一行格式，可空；展示 / 上传服务器用） */
    String publicKey = "";
    long createdAt;
    /** 云同步 LWW 的依据（毫秒时间戳）；编辑保存时刷新 */
    long updatedAt = 0;

    SshKeyEntry() {
        this.id = UUID.randomUUID().toString();
        this.createdAt = System.currentTimeMillis();
    }

    JSONObject toJson() throws JSONException {
        JSONObject object = new JSONObject();
        object.put("id", id);
        object.put("name", JumpStore.text(name));
        object.put("algorithm", JumpStore.text(algorithm));
        object.put("fingerprint", JumpStore.text(fingerprint));
        object.put("privateEnc", JumpStore.text(privateEnc));
        object.put("publicKey", JumpStore.text(publicKey));
        object.put("createdAt", createdAt);
        object.put("updatedAt", updatedAt);
        return object;
    }

    static SshKeyEntry fromJson(JSONObject object) {
        SshKeyEntry entry = new SshKeyEntry();
        String id = object.optString("id", "");
        if (!id.isEmpty()) {
            entry.id = id;
        }
        entry.name = object.optString("name", "");
        entry.algorithm = object.optString("algorithm", "");
        entry.fingerprint = object.optString("fingerprint", "");
        entry.privateEnc = object.optString("privateEnc", "");
        entry.publicKey = object.optString("publicKey", "");
        entry.createdAt = object.optLong("createdAt", 0);
        entry.updatedAt = object.optLong("updatedAt", 0);
        return entry;
    }
}
