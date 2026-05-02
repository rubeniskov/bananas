# 5-Inch LCD Configuration for BananaPro

## Overview
This document provides the configuration details for enabling the 5-inch LCD display on the BananaPro board.

## LCD Specifications (5-inch)
- **Resolution**: 800x480 pixels
- **Color Depth**: 24-bit (RGB888)
- **Pixel Clock**: 30MHz (30000 KHz)
- **Interface**: Parallel RGB

## U-Boot Configuration

### Video Mode Settings
```
CONFIG_VIDEO_LCD_MODE="x:800,y:480,depth:24,pclk_khz:30000,le:40,ri:40,up:29,lo:13,hs:48,vs:3,sync:3,vmode:0"
```

### Timing Breakdown

| Parameter | Value | Description |
|-----------|-------|-------------|
| `x` | 800 | Horizontal resolution (pixels) |
| `y` | 480 | Vertical resolution (pixels) |
| `depth` | 24 | Color depth (bits) - RGB888 |
| `pclk_khz` | 30000 | Pixel clock frequency (30 MHz) |
| `le` | 40 | Left margin (horizontal back porch) |
| `ri` | 40 | Right margin (horizontal front porch) |
| `up` | 29 | Top margin (vertical back porch) |
| `lo` | 13 | Bottom margin (vertical front porch) |
| `hs` | 48 | Horizontal sync length |
| `vs` | 3 | Vertical sync length |
| `sync` | 3 | Sync flags |
| `vmode` | 0 | Video mode flags |

### GPIO Configuration

```
CONFIG_VIDEO_LCD_POWER="PH12"      # LCD power control
CONFIG_VIDEO_LCD_BL_EN="PH8"       # Backlight enable
CONFIG_VIDEO_LCD_BL_PWM="PB2"      # Backlight PWM control
```

## Kernel Device Tree Configuration

For mainline Linux kernel (panel-simple driver), convert to `drm_display_mode`:

```c
static const struct drm_display_mode bananapi_5inch_lcd = {
    .clock = 30000,                    // pclk_khz
    .hdisplay = 800,                   // x
    .hsync_start = 800 + 40,           // x + ri = 840
    .hsync_end = 800 + 40 + 48,        // x + ri + hs = 888
    .htotal = 800 + 40 + 48 + 40,      // x + ri + hs + le = 928
    .vdisplay = 480,                   // y
    .vsync_start = 480 + 13,           // y + lo = 493
    .vsync_end = 480 + 13 + 3,         // y + lo + vs = 496
    .vtotal = 480 + 13 + 3 + 29,       // y + lo + vs + up = 525
    .vrefresh = 60,
};
```

### Device Tree Overlay Snippet (panel-simple)

The Yocto build now ships `overlays/bpi-m1p-lcd.dtbo`, which instantiates the
`bananapi,s070wv20-ct16` panel together with the backlight and EDT touchscreen.
If you need to customise it manually, mirror the following structure:

```dts
lcd_panel: lcd-panel {
    compatible = "bananapi,s070wv20-ct16", "simple-panel";
    connector-type = "dpi";
    data-mapping = "rgb888";
    backlight = <&lcd_backlight>;
    enable-gpios = <&pio 7 9 GPIO_ACTIVE_HIGH>; /* PH9 */

    panel-timing {
        clock-frequency = <30000000>;
        hactive = <800>;
        hfront-porch = <40>;
        hback-porch = <40>;
        hsync-len = <48>;
        vactive = <480>;
        vfront-porch = <13>;
        vback-porch = <29>;
        vsync-len = <3>;
    };
};
```

This maps the U-Boot timing string to the Linux `struct drm_display_mode`, so
KMS/DRM drivers (Weston, kmscube, etc.) can light up the panel without extra
userspace tweaks.

## FEX to U-Boot Conversion Reference

Based on the conversion table from sunxi-linux wiki:

| U-Boot Parameter | FEX Equivalent | Notes |
|------------------|----------------|-------|
| `x` | `lcd_x` | Horizontal resolution |
| `y` | `lcd_y` | Vertical resolution |
| `depth` | `lcd_frm` | 0=24bit, 1=18bit |
| `pclk_khz` | `lcd_dclk_freq * 1000` | Pixel clock |
| `hs` | `lcd_hv_hspw` | H-sync pulse width |
| `vs` | `lcd_hv_vspw` | V-sync pulse width |
| `le` | `lcd_hbp - hs` | Left margin |
| `ri` | `lcd_ht - lcd_x - lcd_hbp` | Right margin |
| `up` | `lcd_vbp - vs` | Top margin |
| `lo` | `(lcd_vt / 2) - lcd_y - lcd_vbp` | Bottom margin (A20); for sun8i use `lcd_vt - lcd_y - lcd_vbp` |

### Worked Example (from `banana_pro_5lcd.fex`)

1. Extract the LCD block from the FEX and note:
    - `lcd_x = 800`, `lcd_y = 480`
    - `lcd_frm = 0` (RGB888)
    - `lcd_dclk_freq = 30000` (kHz)
    - `lcd_hv_hspw = 48`, `lcd_hv_vspw = 3`
    - `lcd_hbp = 88`, `lcd_ht = 928`
    - `lcd_vbp = 32`, `lcd_vt = 1050`
2. Apply the rules above:
    - `le = lcd_hbp - hs = 88 - 48 = 40`
    - `ri = lcd_ht - lcd_x - lcd_hbp = 928 - 800 - 88 = 40`
    - `up = lcd_vbp - vs = 32 - 3 = 29`
    - `lo = (lcd_vt / 2) - lcd_y - lcd_vbp = 525 - 480 - 32 = 13`
3. Assemble the final string:
    ```
    CONFIG_VIDEO_LCD_MODE="x:800,y:480,depth:24,pclk_khz:30000,le:40,ri:40,up:29,lo:13,hs:48,vs:3,sync:3,vmode:0"
    ```

This matches the entry from the Linux-sunxi bulk conversion table and gives confidence that the timing budget is correct for the 5-inch TFT.

> Tip: to automate these calculations for any FEX file, run `scripts/fex_to_uboot.py <path-to-fex>` and the helper will print both the `CONFIG_VIDEO_LCD_MODE` string and a ready-to-paste `drm_display_mode` JSON block.

## Building U-Boot with LCD Support

### 1. Configure U-Boot

Edit your U-Boot defconfig or add to `configs/bananapi_m1_plus_defconfig`:

```makefile
CONFIG_VIDEO_SUNXI=y
CONFIG_VIDEO_LCD_MODE="x:800,y:480,depth:24,pclk_khz:30000,le:40,ri:40,up:29,lo:13,hs:48,vs:3,sync:3,vmode:0"
CONFIG_VIDEO_LCD_POWER="PH12"
CONFIG_VIDEO_LCD_BL_EN="PH8"
CONFIG_VIDEO_LCD_BL_PWM="PB2"
CONFIG_VIDEO_LCD_BL_PWM_ACTIVE_LOW=n
CONFIG_CONSOLE_MUX=y
CONFIG_VIDEO=y
CONFIG_DM_VIDEO=y
CONFIG_DISPLAY=y
```

### 2. Build U-Boot

```bash
cd /path/to/u-boot
make ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- bananapi_m1_plus_defconfig
make ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- menuconfig
# Navigate to Device Drivers -> Graphics support -> Enable LCD support
make ARCH=arm CROSS_COMPILE=arm-linux-gnueabihf- -j$(nproc)
```

## Installation

1. **Flash U-Boot to SD card:**
   ```bash
   sudo dd if=u-boot-sunxi-with-spl.bin of=/dev/sdX bs=1024 seek=8
   ```

2. **Apply the device tree overlay** (if using mainline kernel)

3. **Test the display** after boot

## Verification

After booting, check that the display is detected:

```bash
# Check kernel messages
dmesg | grep -i lcd
dmesg | grep -i display

# Check framebuffer
fbset -i

# Display test pattern
cat /dev/urandom > /dev/fb0

# Exercise the DRM stack (Wayland session)
export XDG_RUNTIME_DIR=/run/user/0
weston --backend=drm-backend.so --tty=1 &
sleep 3 && kmscube
```

## Troubleshooting

### No Display Output
- Verify GPIO connections (PH12, PH8, PB2)
- Check power supply (5V, sufficient current)
- Verify LCD cable connections

### Display Artifacts
- Adjust pixel clock (`pclk_khz`)
- Fine-tune timing margins (`le`, `ri`, `up`, `lo`)

### Backlight Issues
- Check `CONFIG_VIDEO_LCD_BL_PWM_ACTIVE_LOW` setting
- Verify PB2 PWM functionality

## References

- [Linux Sunxi LCD Wiki](https://linux-sunxi.org/LCD)
- [U-Boot Video Configuration](https://u-boot.readthedocs.io/en/latest/board/allwinner/sunxi.html)
- BananaPi 5" LCD FEX files: `banana_pi_5lcd.fex`, `banana_pro_5lcd.fex`
