package com.anyboot.remote;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;

import java.io.ByteArrayOutputStream;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.SecureRandom;

import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/**
 * 敏感字段的落盘加密：Android Keystore 里的 AES-256-GCM 密钥包一层。
 *
 * <p>SSH 密码 / 私钥 PEM / 密钥口令都走这里：配置文件里只存
 * {@code base64(iv || ciphertext)}，密钥本体不出 Keystore、不进备份。
 * 首次调用时生成密钥（一次性），之后复用。
 */
final class KeyVault {

    private static final String STORE = "AndroidKeyStore";
    private static final String ALIAS = "anyboot_vault_key";
    private static final String TRANSFORM = "AES/GCM/NoPadding";
    private static final int IV_LEN = 12;
    private static final int TAG_BITS = 128;

    private KeyVault() {
    }

    /** 明文 → base64(iv||ct)；空串原样返回（没填密码就不存东西） */
    static String encrypt(String plain) {
        String value = JumpStore.text(plain);
        if (value.isEmpty()) {
            return "";
        }
        try {
            byte[] iv = new byte[IV_LEN];
            new SecureRandom().nextBytes(iv);
            Cipher cipher = Cipher.getInstance(TRANSFORM);
            cipher.init(Cipher.ENCRYPT_MODE, key(), new GCMParameterSpec(TAG_BITS, iv));
            byte[] ciphertext = cipher.doFinal(value.getBytes("UTF-8"));
            ByteArrayOutputStream out = new ByteArrayOutputStream(iv.length + ciphertext.length);
            out.write(iv);
            out.write(ciphertext);
            return Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP);
        } catch (Exception error) {
            // 不落盘明文 —— 宁可让用户重输一次密码。但**必须留痕**：
            // 异常被静默吞掉，正是「保存密码无声失败」这类问题反复查不出的原因。
            TerminalActivity.tlog("KeyVault.encrypt 失败: " + error);
            return "";
        }
    }

    /** base64(iv||ct) → 明文；解不开（换机恢复 / 损坏）给空串 */
    static String decrypt(String blob) {
        String value = JumpStore.text(blob);
        if (value.isEmpty()) {
            return "";
        }
        try {
            byte[] all = Base64.decode(value, Base64.NO_WRAP);
            if (all.length <= IV_LEN) {
                return "";
            }
            Cipher cipher = Cipher.getInstance(TRANSFORM);
            cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(TAG_BITS, all, 0, IV_LEN));
            byte[] plain = cipher.doFinal(all, IV_LEN, all.length - IV_LEN);
            return new String(plain, "UTF-8");
        } catch (Exception error) {
            // 两种情况都会到这：密钥库那把钥匙换过（旧密文作废）/ 密文损坏。
            // 都要看得见，否则用户眼里的现象只是「密码怎么又没了」。
            TerminalActivity.tlog("KeyVault.decrypt 失败: " + error);
            return "";
        }
    }

    /**
     * 取那把 AES-256 密钥；**只在没有时才生成**。
     *
     * <p>以前这里是无条件 {@code generateKey()}，注释还写着「已存在时幂等」——
     * 那是错的：Android Keystore 对已存在的 alias 是**覆盖**。
     * 每调一次 encrypt/decrypt 就换一把密钥，等于把刚写进去的密文当场作废：
     * 密码 / 私钥 PEM / 密钥口令全部变成解不开的垃圾，
     * 表现就是「保存密码永远不生效、每次连接都要临时密码」。
     */
    private static synchronized SecretKey key() throws Exception {
        KeyStore keyStore = KeyStore.getInstance(STORE);
        keyStore.load(null);
        KeyStore.Entry cached = keyStore.getEntry(ALIAS, null);
        if (cached instanceof KeyStore.SecretKeyEntry) {
            return ((KeyStore.SecretKeyEntry) cached).getSecretKey();
        }
        KeyGenParameterSpec spec = new KeyGenParameterSpec.Builder(
                ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(false)   // IV 自己带（12B 随机）
                .build();
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE);
        generator.init(spec);
        return generator.generateKey();
    }

    /** SHA-256 指纹（SHA256:base64 格式，OpenSSH 口径），给密钥卡 / 主机指纹用 */
    static String sha256Fingerprint(byte[] data) {
        try {
            byte[] digest = MessageDigest.getInstance("SHA-256").digest(data);
            return "SHA256:" + Base64.encodeToString(digest, Base64.NO_WRAP | Base64.NO_PADDING)
                    .replace("=", "");
        } catch (Exception ignored) {
            return "";
        }
    }
}
