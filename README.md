# BanaNAS Project

  This is about for converting a Banana Pro (BPI-M1P) SoC into a usefull [NAS](https://es.wikipedia.org/wiki/Network_access_server) for
  serving volumes over a network using [NFS](https://en.wikipedia.org/wiki/Network_File_System) protocol for simplicity

  Since the Banana Pro supports and attached LCD, the Linux kernel may require specific patches and configurations
  changes depending on the LCD model you use, refer to this [LCD documentation](docs/LCD_5INCH_CONFIGURATION.md) for further details.

## Host Requirements

Bitbake requires several host tools that are not fully provided by the `pixi` environment. Please install them on your host system:

### Linux (Ubuntu/Debian)
```bash
sudo apt-get update
sudo apt-get install chrpath cpio diffstat hostname rpcsvc-proto
```

### Linux (Arch)
```bash
sudo pacman -S chrpath cpio diffstat inetutils rpcsvc-proto
```

### macOS (Homebrew)
*Note: Yocto builds are recommended inside a Linux VM or Docker container on macOS.*
```bash
brew install chrpath cpio diffstat rpcgen
```

### Windows (WSL2)
*Yocto/Bitbake is supported on Windows via WSL2 (Ubuntu is recommended).*
```bash
sudo apt-get update
sudo apt-get install chrpath cpio diffstat hostname rpcsvc-proto
```

## Building the Image

This project uses `pixi` and `kas` for the build process. To build the NAS image:

```bash
# Verify your environment is set up
pixi run info

# Run the build
pixi run build
```

## Login

- **SSH** is key-only. Drop your public key into `layers/meta-bananas/recipes-core/images/files/authorized_keys` before building. `PermitRootLogin prohibit-password` (the OpenSSH default) is unchanged, so password auth over SSH is blocked by design.
- **Serial / LCD console** uses a password. **Default password is `1234`.**

### Changing the root password

The build picks the SHA-512 hash up from `ROOT_PASSWORD_HASH`. Generate a hash:

```bash
openssl passwd -6 -salt $(openssl rand -hex 8)
```

Override at build time without touching the recipe (the variable is in `kas.yml`'s `BB_ENV_PASSTHROUGH_ADDITIONS`):

```bash
export ROOT_PASSWORD_HASH='$6$your-salt$your-hash'
pixi run build
```

For CI/CD, inject `ROOT_PASSWORD_HASH` as a masked secret. The default in the recipe is only used when no env value is set.

To make the change permanent in the repo, edit `ROOT_PASSWORD_HASH` in `layers/meta-bananas/recipes-core/images/bananas-image.bb`.
