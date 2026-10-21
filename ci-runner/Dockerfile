# The self-hosted GitHub Actions runner that benchmarks Helium (label `bench`).
#
# Jobs run inside this container as the unprivileged user `runner`, so they
# see only what the image and the mounts in compose.ci.yaml give them. The
# toolchain versions below are part of what is measured: change them
# deliberately, and rebuild (`docker compose -f compose.yaml -f compose.ci.yaml build`).

FROM ubuntu:24.04

# Helium's pinned nightly (its rust-toolchain.toml) and `rustc_toolchain` in
# runner/suite.ci.json.
ARG RUST_TOOLCHAIN=nightly-2026-08-19
# The Z3 Silicon runs with.
ARG Z3_VERSION=4.8.7
ARG Z3_ASSET=z3-4.8.7-x64-ubuntu-16.04
# Version of the GitHub runner, e.g. 2.329.0; empty for the latest release.
# The runner updates itself afterwards.
ARG RUNNER_VERSION=

ENV DEBIAN_FRONTEND=noninteractive
# build-essential: Helium's and the runner crate's builds. A JDK, not a JRE:
# runner/silicon/SiliconWarm.java is started as a source file. libicu,
# libkrb5, zlib: the GitHub runner. libgomp: Z3.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git jq unzip build-essential pkg-config \
      openjdk-21-jdk-headless libgomp1 libicu74 libkrb5-3 zlib1g \
 && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL -o /tmp/z3.zip "https://github.com/Z3Prover/z3/releases/download/z3-${Z3_VERSION}/${Z3_ASSET}.zip" \
 && unzip -q /tmp/z3.zip -d /opt \
 && mv "/opt/${Z3_ASSET}" /opt/z3 \
 && rm /tmp/z3.zip
ENV Z3_EXE=/opt/z3/bin/z3

RUN useradd --create-home --shell /bin/bash runner
WORKDIR /home/runner/actions-runner
RUN version="${RUNNER_VERSION:-$(curl -fsSL https://api.github.com/repos/actions/runner/releases/latest | jq -r .tag_name | sed 's/^v//')}" \
 && curl -fsSL -o runner.tar.gz "https://github.com/actions/runner/releases/download/v${version}/actions-runner-linux-x64-${version}.tar.gz" \
 && tar xzf runner.tar.gz \
 && rm runner.tar.gz
# Mount points of the volumes in compose.ci.yaml, owned by `runner` so the
# volumes are too.
RUN mkdir -p /home/runner/_work /home/runner/.cargo/registry /home/runner/.cargo/git \
      /opt/bench/helium-target /opt/bench/runner-target \
 && chown -R runner:runner /home/runner /opt/bench

USER runner
ENV PATH=/home/runner/.cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
 && rustup toolchain install "${RUST_TOOLCHAIN}" --profile minimal --component rustfmt,clippy

COPY --chmod=755 entrypoint.sh /entrypoint.sh
ENTRYPOINT ["/entrypoint.sh"]
