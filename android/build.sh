#!/usr/bin/env bash
# CineVault Android 轻量 APK 构建脚本（不依赖 Gradle / AndroidX / 网络下载）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$LOCALAPPDATA/Android/Sdk}}"
JAVA_HOME="${JAVA_HOME:-C:/Program Files/Android/Android Studio/jbr}"
BT="$SDK/build-tools/36.1.0"
ANDROID_JAR="$SDK/platforms/android-35/android.jar"
JAVA="$JAVA_HOME/bin/java.exe"
JAVAC="$JAVA_HOME/bin/javac.exe"
KEYTOOL="$JAVA_HOME/bin/keytool.exe"
AAPT2="$BT/aapt2.exe"
ZIPALIGN="$BT/zipalign.exe"
D8="$BT/lib/d8.jar"
APKSIGNER="$BT/lib/apksigner.jar"
PYTHON="${PYTHON:-python}"

for f in "$JAVA" "$JAVAC" "$AAPT2" "$ZIPALIGN" "$ANDROID_JAR" "$D8" "$APKSIGNER"; do
  if [ ! -f "$f" ]; then
    echo "缺少构建工具: $f" >&2
    exit 1
  fi
done

rm -rf build
mkdir -p build/gen build/classes build/dex

echo "[1/7] 编译资源"
"$AAPT2" compile --dir res -o build/res.zip

echo "[2/7] 链接资源与 Manifest"
"$AAPT2" link \
  -o build/unsigned.apk \
  -I "$ANDROID_JAR" \
  --manifest AndroidManifest.xml \
  --java build/gen \
  --min-sdk-version 26 \
  --target-sdk-version 34 \
  --auto-add-overlay \
  build/res.zip

echo "[3/7] 编译 Java"
find java build/gen -name '*.java' -print > build/sources.txt
"$JAVAC" -encoding UTF-8 -source 11 -target 11 \
  -classpath "$ANDROID_JAR" \
  -d build/classes @build/sources.txt

echo "[4/7] D8 生成 classes.dex"
find build/classes -name '*.class' -print > build/classes.txt
"$JAVA" -cp "$D8" com.android.tools.r8.D8 \
  --lib "$ANDROID_JAR" --min-api 26 --release \
  --output build/dex $(cat build/classes.txt)

echo "[5/7] 写入 APK"
"$PYTHON" - <<'PY'
import os, shutil, zipfile
src = os.path.join('build', 'unsigned.apk')
dst = os.path.join('build', 'unaligned.apk')
shutil.copyfile(src, dst)
with zipfile.ZipFile(dst, 'a', zipfile.ZIP_DEFLATED) as z:
    z.write(os.path.join('build', 'dex', 'classes.dex'), 'classes.dex')
PY

echo "[6/7] 对齐"
"$ZIPALIGN" -f -p 4 build/unaligned.apk build/aligned.apk

echo "[7/7] 签名"
# 签名库必须放在 build/ 之外：build 每次清空，放里面会导致每次重新生成密钥，
# 新 APK 就无法覆盖安装旧版本（Android 要求同包名同签名才能升级）。
KEYSTORE="$ROOT/keystore/cinevault-mobile.jks"
PASSFILE="$ROOT/keystore/keystore.pass"
KEYPASSFILE="$ROOT/keystore/keystore.keypass"
mkdir -p "$ROOT/keystore"
chmod 700 "$ROOT/keystore" 2>/dev/null || true

if [ -n "${CINEVAULT_KEYSTORE_PASS:-}" ]; then
  printf '%s\n' "$CINEVAULT_KEYSTORE_PASS" > "$PASSFILE"
elif [ ! -f "$PASSFILE" ] || [ ! -s "$PASSFILE" ]; then
  if [ -f "$KEYSTORE" ]; then
    # 旧脚本使用公开固定口令。首次迁移时先备份，再把库口令和私钥口令都改为随机值。
    cp -p "$KEYSTORE" "$KEYSTORE.legacy.bak"
    "$PYTHON" -c 'import secrets,sys; sys.stdout.write(secrets.token_urlsafe(32) + "\n")' > "$PASSFILE"
    NEWPASS="$(tr -d '\r\n' < "$PASSFILE")"
    "$KEYTOOL" -storepasswd -keystore "$KEYSTORE" \
      -storepass cinevault -new "$NEWPASS" >/dev/null
    if "$KEYTOOL" -keypasswd -alias cinevault -keystore "$KEYSTORE" \
      -storepass "$NEWPASS" -keypass cinevault -new "$NEWPASS" >/dev/null \
      && "$KEYTOOL" -list -keystore "$KEYSTORE" -storepass:file "$PASSFILE" >/dev/null; then
      # 验证新口令可打开后才删除旧口令备份，避免“公开口令副本”长期留在磁盘。
      rm -f "$KEYSTORE.legacy.bak"
      echo "已将旧签名密钥迁移为随机口令，并验证通过。"
    else
      mv -f "$KEYSTORE.legacy.bak" "$KEYSTORE"
      echo "签名密钥口令迁移失败，已恢复旧库。" >&2
      exit 1
    fi
  else
    "$PYTHON" -c 'import secrets,sys; sys.stdout.write(secrets.token_urlsafe(32) + "\n")' > "$PASSFILE"
  fi
fi
# apksigner 对同一个口令文件先后读取两次会在 EOF 处失败，私钥口令单独给一份。
cp "$PASSFILE" "$KEYPASSFILE"
chmod 600 "$PASSFILE" "$KEYPASSFILE" 2>/dev/null || true
# Java 不认 Git Bash 的 /h/... 路径，交给 apksigner 前转成 Windows 路径。
PASSFILE_WIN="$(cygpath -w "$PASSFILE")"
KEYPASSFILE_WIN="$(cygpath -w "$KEYPASSFILE")"

if [ ! -s "$PASSFILE" ]; then
  echo "签名口令文件为空: $PASSFILE" >&2
  exit 1
fi

if [ ! -f "$KEYSTORE" ]; then
  "$KEYTOOL" -genkeypair -v \
    -keystore "$KEYSTORE" \
    -storepass:file "$PASSFILE" -keypass:file "$PASSFILE" \
    -alias cinevault -keyalg RSA -keysize 2048 -validity 10000 \
    -dname "CN=CineVault Mobile,O=CineVault,C=CN" >/dev/null 2>&1
fi
"$JAVA" -jar "$APKSIGNER" sign \
  --ks "$KEYSTORE" \
  --ks-key-alias cinevault \
  --ks-pass "file:$PASSFILE_WIN" \
  --key-pass "file:$KEYPASSFILE_WIN" \
  --out build/CineVaultMobile.apk \
  build/aligned.apk

"$JAVA" -jar "$APKSIGNER" verify --print-certs build/CineVaultMobile.apk
echo
echo "APK: $ROOT/build/CineVaultMobile.apk"
ls -lh build/CineVaultMobile.apk
