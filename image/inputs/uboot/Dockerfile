# Cross toolchain for image/uboot-build.sh (U-Boot v2025.10 for the A33). Built once as thinframe-uboot.
FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential gcc-arm-linux-gnueabihf bison flex bc libssl-dev libgnutls28-dev \
    python3 python3-dev python3-setuptools python3-pyelftools swig device-tree-compiler \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
