#!/bin/bash
# Runs as root, once, on the droplet the hub makes to build the phone's image: the newest Android SDK tools, emulator
# and Google Play system image on a Pixel 8, the screen stream's server, and /usr/local/bin/phone-boot, which the
# runner calls to boot the phone. The first boot happens here and is saved as the emulator's quick-boot state, so a
# phone from the image resumes instead of booting. The droplet powers itself off when it's done, which is how the hub
# knows to snapshot it; if anything fails it stays up and the hub gives up on it.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive HOME=/root
SCRCPY=5.0.1
SCRCPY_SHA256=764eb6f79811d5211fe9df341120882ba9994c7a61b897d7bf3fb662e53bc536

apt-get update -q >/dev/null
apt-get install -y -q default-jdk-headless unzip curl libpulse0 libgl1 libnss3 libxcomposite1 libxcursor1 \
  libxdamage1 libxi6 libxtst6 libxrandr2 libx11-xcb1 libxkbfile1 libxkbcommon0 libasound2t64 libbsd0 >/dev/null
export ANDROID_HOME=/opt/sdk
mkdir -p $ANDROID_HOME/cmdline-tools && cd /tmp
clt=$(curl -sf https://dl.google.com/android/repository/repository2-3.xml | grep -oE 'commandlinetools-linux-[0-9]+_latest\.zip' | sort -V | tail -1)
curl -sfL -o clt.zip "https://dl.google.com/android/repository/$clt"
unzip -q -o clt.zip -d $ANDROID_HOME/cmdline-tools
mv $ANDROID_HOME/cmdline-tools/cmdline-tools $ANDROID_HOME/cmdline-tools/latest
SM=$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager
(yes || true) | $SM --licenses >/dev/null 2>&1 || true
IMG=$($SM --list 2>/dev/null | grep -oE 'system-images[;/]android-[0-9]+(\.[0-9]+)?[;/]google_apis_playstore[;/]x86_64' | tr / ';' | sort -t- -k2 -V | tail -1)
$SM --install platform-tools emulator "$IMG" >/dev/null
echo no | $ANDROID_HOME/cmdline-tools/latest/bin/avdmanager create avd -n phone -k "$IMG" -d pixel_8 >/dev/null
sed -i 's/^disk.dataPartition.size.*/disk.dataPartition.size=4G/' /root/.android/avd/phone.avd/config.ini

curl -sfL -o /opt/scrcpy-server-v$SCRCPY "https://github.com/Genymobile/scrcpy/releases/download/v$SCRCPY/scrcpy-server-v$SCRCPY"
echo "$SCRCPY_SHA256  /opt/scrcpy-server-v$SCRCPY" | sha256sum -c - >/dev/null

cat > /usr/local/bin/phone-boot <<'SH'
#!/bin/bash
# Boots the phone headless if it isn't running, and waits until Android has finished starting.
export HOME=/root ANDROID_HOME=/opt/sdk PATH=/opt/sdk/platform-tools:$PATH
adb start-server >/dev/null 2>&1
if ! pgrep -f '[q]emu-system' >/dev/null; then
  nohup $ANDROID_HOME/emulator/emulator -avd phone -no-window -no-audio -no-boot-anim -gpu swiftshader_indirect \
    ${PHONE_BOOT_FLAGS:--no-snapshot-save} > /tmp/emulator.log 2>&1 < /dev/null &
fi
for _ in $(seq 1 150); do
  [ "$(adb -e shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && exit 0
  sleep 2
done
echo "the phone didn't finish booting; see /tmp/emulator.log" >&2; exit 1
SH
chmod +x /usr/local/bin/phone-boot

# First boot (slow: Android sets itself up), then a clean stop that saves the quick-boot state.
PHONE_BOOT_FLAGS=" " /usr/local/bin/phone-boot
sleep 30
/opt/sdk/platform-tools/adb -e emu kill >/dev/null
for _ in $(seq 1 60); do pgrep -f '[q]emu-system' >/dev/null || break; sleep 2; done
/opt/sdk/platform-tools/adb kill-server >/dev/null 2>&1 || true
sync
poweroff
