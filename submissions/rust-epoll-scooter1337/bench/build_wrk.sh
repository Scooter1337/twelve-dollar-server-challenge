#!/usr/bin/env bash
# Benchmark tool only. Both implementations use identical CLOCK_MONOTONIC timing.
set -euo pipefail
mkdir -p /bench/wrk-mono
cd /bench/wrk-mono
curl -fsSL --retry 3 https://codeload.github.com/wg/wrk/tar.gz/refs/tags/4.2.0 -o /bench/wrk.tar.gz
printf '%s  %s\n' e255f696bff6e329f5d19091da6b06164b8d59d62cb9e673625bdcd27fe7bdad /bench/wrk.tar.gz | sha256sum -c -
tar -xzf /bench/wrk.tar.gz --strip-components=1
python3 - <<'PY'
from pathlib import Path
p=Path('src/wrk.c');s=p.read_text()
old='''static uint64_t time_us() {
    struct timeval t;
    gettimeofday(&t, NULL);
    return (t.tv_sec * 1000000) + t.tv_usec;
}'''
new='''static uint64_t time_us() {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return ((uint64_t)t.tv_sec * 1000000) + (t.tv_nsec / 1000);
}'''
assert s.count(old)==1,'pinned wrk timing function changed'
s=s.replace('#include "main.h"','#include "main.h"\n#include <time.h>')
p.write_text(s.replace(old,new))
PY
CFLAGS='-I/usr/include/luajit-2.1' make -j2 WITH_LUAJIT=/usr WITH_OPENSSL=/usr VER=4.2.0-monotonic
