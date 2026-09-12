# syntax=docker/dockerfile:1.7
ARG BASE_IMAGE=ghcr.io/cpeter1207/rpt-advanced-quality-debian13:latest
FROM ${BASE_IMAGE}

ARG TARGETARCH
ARG RUSTUP_INIT_VERSION=1.28.2
ARG RUST_STABLE=1.85.0
ARG RUST_NIGHTLY=nightly-2025-02-20
ARG CARGO_LLVM_COV_VERSION=0.6.21

ENV RUSTUP_HOME=/opt/rustup
ENV CARGO_HOME=/opt/cargo
ENV PATH=/opt/cargo/bin:${PATH}

RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
		autopkgtest ca-certificates cargo curl debhelper docker-cli dpkg-dev iproute2 pkg-config rustc && \
	rm -rf /var/lib/apt/lists/*

COPY --from=samplerate_adapter_debs librptadv-samplerate-adapter1_*.deb /tmp/rptadv-samplerate-adapter/
COPY --from=samplerate_adapter_debs librptadv-samplerate-adapter-dev_*.deb /tmp/rptadv-samplerate-adapter/

RUN dpkg -i /tmp/rptadv-samplerate-adapter/librptadv-samplerate-adapter1_*.deb \
		/tmp/rptadv-samplerate-adapter/librptadv-samplerate-adapter-dev_*.deb && \
	pkg-config --atleast-version=0.1.0~alpha1 rptadv_samplerate_adapter && \
	rm -rf /tmp/rptadv-samplerate-adapter

RUN case "${TARGETARCH}" in \
		amd64) rustup_host=x86_64-unknown-linux-gnu; rustup_sha=20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c ;; \
		arm64) rustup_host=aarch64-unknown-linux-gnu; rustup_sha=e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c ;; \
		*) echo "unsupported architecture: ${TARGETARCH}" >&2; exit 1 ;; \
	esac; \
	curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
		"https://static.rust-lang.org/rustup/archive/${RUSTUP_INIT_VERSION}/${rustup_host}/rustup-init" \
		--output /tmp/rustup-init; \
	echo "${rustup_sha}  /tmp/rustup-init" | sha256sum --check --status; \
	chmod 0755 /tmp/rustup-init; \
	RUSTUP_INIT_SKIP_PATH_CHECK=yes /tmp/rustup-init -y --no-modify-path --default-toolchain none; \
	rm -f /tmp/rustup-init

RUN rustup toolchain install "${RUST_STABLE}" --profile minimal --component clippy --component rustfmt && \
	rustup toolchain install "${RUST_NIGHTLY}" --profile minimal --component llvm-tools-preview && \
	rustup default "${RUST_STABLE}" && \
	cargo +"${RUST_NIGHTLY}" install cargo-llvm-cov --version "${CARGO_LLVM_COV_VERSION}" --locked && \
	cargo +"${RUST_NIGHTLY}" llvm-cov --version

WORKDIR /workspace
