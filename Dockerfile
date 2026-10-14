# syntax=docker/dockerfile:1
# The wasm bundle is platform-independent: build it on the build machine's platform, and only
# make the nginx stage match the cluster (linux/amd64).
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS build
ARG TRUNK_VERSION=0.21.14
RUN rustup target add wasm32-unknown-unknown \
 && case "$(uname -m)" in aarch64) t=aarch64-unknown-linux-gnu ;; *) t=x86_64-unknown-linux-gnu ;; esac \
 && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${t}.tar.gz" | tar -xz -C /usr/local/bin
WORKDIR /src
COPY . .
RUN trunk build --release

# Stable branch (gets security fixes).
FROM nginxinc/nginx-unprivileged:1.28-alpine
COPY docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /src/target/web /usr/share/nginx/html
EXPOSE 8080
