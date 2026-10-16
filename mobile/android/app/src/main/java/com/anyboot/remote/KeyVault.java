package com.anyboot.remote;

import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;

import java.io.ByteArrayOutputStream;
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
        } catch (Exception ignored) {
            // Keystore 出问题时不落盘明文 —— 宁可让用户重输一次密码
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
        } catch (Exception ignored) {
            return "";
        }
    }

    private static SecretKey key() throws Exception {
        KeyGenParameterSpec spec = new KeyGenParameterSpec.Builder(
                ALIAS, KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(false)   // IV 自己带（12B 随机）
                .build();
        KeyGenerator generator = KeyGenerator.getInstance(
                KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(spec);
        // 密钥已存在时 generateKey 幂等（keystore 里还是同一把）
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
