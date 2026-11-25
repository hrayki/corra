#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive HOME=/root ANDROID_HOME=/opt/sdk
SCRCPY=5.0.1
SCRCPY_SHA256=764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536
SM=$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager
DONE=/opt/sdk/.corra-phone-ready

if [ -f "$DONE" ] && [ -x /usr/local/bin/phone-boot ]; then
  echo "the phone is already set up"
  exit 0
fi

if [ ! -e /dev/kvm ]; then
  echo "this machine has no /dev/kvm, so it can't run the Android emulator" >&2
  exit 2
fi

apt-get -o DPkg::Lock::Timeout=600 update -q >/dev/null
apt-get -o DPkg::Lock::Timeout=600 install -y -q --no-install-recommends default-jdk-headless unzip curl \
  libpulse0 libgl1 libnss3 libxcomposite1 libxcursor1 libxdamage1 libxi6 libxtst6 libxrandr2 libx11-xcb1 \
  libxkbfile1 libxkbcommon0 libasound2t64 libbsd0 >/dev/null

if [ ! -x "$SM" ]; then
  rm -rf $ANDROID_HOME/cmdline-tools
  mkdir -p $ANDROID_HOME/cmdline-tools
  clt=$(curl -sf https://dl.google.com/android/repository/repository2-3.xml | grep -oE 'commandlinetools-linux-[0-9]+_latest\.zip' | sort -V | tail -1)
  curl -sfL -o /tmp/clt.zip "https://dl.google.com/android/repository/$clt"
  unzip -q -o /tmp/clt.zip -d $ANDROID_HOME/cmdline-tools
  mv $ANDROID_HOME/cmdline-tools/cmdline-tools $ANDROID_HOME/cmdline-tools/latest
  rm -f /tmp/clt.zip
fi
(yes || true) | $SM --licenses >/dev/null 2>&1 || true
IMG=$($SM --list 2>/dev/null | grep -oE 'system-images[;/]android-[0-9]+(\.[0-9]+)?[;/]google_apis_playstore[;/]x86_64' | tr / ';' | sort -t- -k2 -V | tail -1)
$SM --install platform-tools emulator "$IMG" >/dev/null

if [ ! -f /root/.android/avd/phone.ini ]; then
  echo no | $ANDROID_HOME/cmdline-tools/latest/bin/avdmanager create avd -n phone -k "$IMG" -d pixel_8 --force >/dev/null
  sed -i 's/^disk.dataPartition.size.*/disk.dataPartition.size=4G/' /root/.android/avd/phone.avd/config.ini
fi

if ! echo "$SCRCPY_SHA256  /opt/scrcpy-server-v$SCRCPY" | sha256sum -c - >/dev/null 2>&1; then
  curl -sfL -o /opt/scrcpy-server-v$SCRCPY "https://github.com/Genymobile/scrcpy/releases/download/v$SCRCPY/scrcpy-server-v$SCRCPY"
  echo "$SCRCPY_SHA256  /opt/scrcpy-server-v$SCRCPY" | sha256sum -c - >/dev/null
fi

cat > /usr/local/bin/phone-boot <<'SH'
#!/bin/bash
export HOME=/root ANDROID_HOME=/opt/sdk PATH=/opt/sdk/platform-tools:$PATH
adb start-server >/dev/null 2>&1
if ! pgrep -f '[q]emu-system' >/dev/null; then
  rm -f /root/.android/avd/phone.avd/*.lock
  nohup $ANDROID_HOME/emulator/emulator -avd phone -no-window -no-audio -no-boot-anim -gpu swiftshader_indirect \
    ${PHONE_BOOT_FLAGS:--no-snapshot-save} > /tmp/emulator.log 2>&1 < /dev/null &
fi
for _ in $(seq 1 150); do
  [ "$(adb -e shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && exit 0
  sleep 2
done
echo "the phone didn't finish booting; see /tmp/emulator.log" >&2
exit 1
SH
chmod +x /usr/local/bin/phone-boot

pkill -f '[q]emu-system' || true
for _ in $(seq 1 30); do pgrep -f '[q]emu-system' >/dev/null || break; sleep 2; done
PHONE_BOOT_FLAGS=" " /usr/local/bin/phone-boot
sleep 30
/opt/sdk/platform-tools/adb -e emu kill >/dev/null
for _ in $(seq 1 60); do pgrep -f '[q]emu-system' >/dev/null || break; sleep 2; done
/opt/sdk/platform-tools/adb kill-server >/dev/null 2>&1 || true
sync
touch "$DONE"
echo "the phone is set up"
