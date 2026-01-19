# Dockerfile for running Snow-Owl TFTP integration tests
FROM rust:1.92-trixie

# Install required packages
RUN apt-get update && apt-get install -y \
    tftp-hpa \
    netcat-openbsd \
    iproute2 \
    procps \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy the entire project
COPY . .

# Build the TFTP server in release mode
RUN cargo build --release --manifest-path crates/snow-owl-tftp/Cargo.toml

# Set working directory to tests
WORKDIR /app/crates/snow-owl-tftp/tests

# Make the test script executable
RUN chmod +x integration-test.sh

# Run the tests
CMD ["./integration-test.sh"]
