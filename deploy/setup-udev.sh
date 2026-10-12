#!/usr/bin/env sh
# Install udev rule and modprobe blacklist for the RTL2832U dongle.
# Works on Debian/Ubuntu laptops and Raspberry Pi OS. Needs sudo.
set -eu

here="$(cd "$(dirname "$0")" && pwd)"

echo "Installing udev rule -> /etc/udev/rules.d/99-rtlsdr.rules"
sudo install -m 0644 "$here/99-rtlsdr.rules" /etc/udev/rules.d/99-rtlsdr.rules

echo "Installing modprobe blacklist -> /etc/modprobe.d/blacklist-rtlsdr.conf"
sudo install -m 0644 "$here/blacklist-rtlsdr.conf" /etc/modprobe.d/blacklist-rtlsdr.conf

echo "Unloading kernel DVB modules (if loaded)"
for m in rtl2832_sdr dvb_usb_rtl28xxu rtl2832 rtl2830; do
    if lsmod | grep -q "^$m "; then
        sudo modprobe -r "$m" || echo "  could not unload $m (unplug and replug the dongle)"
    fi
done

echo "Reloading udev rules"
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb

echo
echo "Done. Replug the dongle once so the new permissions apply."
if lsusb 2>/dev/null | grep -q "0bda:28[33][28]"; then
    echo "Dongle detected:"
    lsusb | grep "0bda:28[33][28]"
fi
