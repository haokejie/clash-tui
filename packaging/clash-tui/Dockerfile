FROM --platform=$BUILDPLATFORM rust:1.91.0-bookworm

ARG DEBIAN_MIRROR=http://deb.debian.org/debian
ARG DEBIAN_SECURITY_MIRROR=http://deb.debian.org/debian-security
ARG RUSTUP_DIST_SERVER=https://static.rust-lang.org

ENV RUSTUP_DIST_SERVER=$RUSTUP_DIST_SERVER

RUN set -eux; \
  if [ "$DEBIAN_MIRROR" != "http://deb.debian.org/debian" ] || [ "$DEBIAN_SECURITY_MIRROR" != "http://deb.debian.org/debian-security" ]; then \
    sed -i \
      -e "s|http://deb.debian.org/debian-security|$DEBIAN_SECURITY_MIRROR|g" \
      -e "s|http://deb.debian.org/debian|$DEBIAN_MIRROR|g" \
      /etc/apt/sources.list.d/debian.sources; \
  fi; \
  apt-get update \
  && apt-get install -y --no-install-recommends \
    g++-aarch64-linux-gnu \
    g++-x86-64-linux-gnu \
    gcc-aarch64-linux-gnu \
    gcc-x86-64-linux-gnu \
  && rm -rf /var/lib/apt/lists/*; \
  rustup component add clippy rustfmt; \
  rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu

ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
ENV CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++
ENV AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
ENV AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar

WORKDIR /workspace
