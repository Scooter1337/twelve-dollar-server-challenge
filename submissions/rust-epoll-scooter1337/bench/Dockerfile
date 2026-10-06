FROM ubuntu:24.04
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    build-essential clang curl ca-certificates unzip libssl-dev libluajit-5.1-dev luajit sqlite3 jq \
    python3 python3-venv openssl git nodejs nginx libyaml-dev zlib1g-dev libffi-dev libgmp-dev tzdata libncurses-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.94.0 \
    && chmod o+x /root && chmod -R a+rX /root/.rustup /root/.cargo/bin && chmod a+r /root/.cargo/env
ENV PATH=/root/.cargo/bin:$PATH
RUN useradd -m -u 1001 bench
WORKDIR /bench
