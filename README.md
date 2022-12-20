# Konachan Popular (Rust)

This repository contains the source code of the backend program running the Telegram channel [@KonachanPopular](https://t.me/KonachanPopular).

<p align="center">
   <img src="https://user-images.githubusercontent.com/21986859/208772514-5c0d1c8d-6132-4dee-931c-9f2cca157ec5.png"/>
</p>

## Run in a Container

You will obviously first have to have an OCI-compatible container runtime like Podman or Docker installed. Then, pull and run the container:

```shell
sudo podman run -e TELOXIDE_TOKEN=$TELOXIDE_TOKEN -e TELOXIDE_CHAT_ID=$TELOXIDE_CHAT_ID ghcr.io/k4yt3x/konachan-popular:1.1.0
```

You can pass the settings either through environment variables or arguments. For details, see the help page of the binary:

```shell
sudo podman run ghcr.io/k4yt3x/konachan-popular:1.1.0 -h
```

## Run From Source

First, you'll need to clone and build this program. For this step, you will need `cargo` to be installed and the `rustc` compiler available.

```shell
git clone https://github.com/k4yt3x/konachan-popular-rust
cd konachan-popular-rust
cargo build --release
```

After the binary is built, you can either run it directly:

```shell
./target/release/konachan-popular -c [CHAT_ID] -t [TOKEN]
```
