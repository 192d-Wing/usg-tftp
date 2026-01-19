#!/bin/bash
# run-docker-tests.sh - Run TFTP integration tests in Docker container

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}================================================${NC}"
echo -e "${BLUE}  Snow-Owl TFTP Docker Integration Tests${NC}"
echo -e "${BLUE}================================================${NC}"
echo ""

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    echo -e "${RED}ERROR: Docker is not installed${NC}"
    echo "Please install Docker from https://www.docker.com/get-started"
    exit 1
fi

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    echo -e "${RED}ERROR: Docker daemon is not running${NC}"
    echo "Please start Docker and try again"
    exit 1
fi

echo -e "${BLUE}Building Docker image...${NC}"
cd "$(dirname "$0")"

# Build the Docker image
docker build -t snow-owl-tftp-test -f Dockerfile.integration ../../.. || {
    echo -e "${RED}ERROR: Failed to build Docker intergration image${NC}"
    exit 1
}

docker build -t snow-owl-tftp-benchmarks -f Dockerfile.bench ../../.. || {
    echo -e "${RED}ERROR: Failed to build Docker benchmark image${NC}"
    exit 1
}

echo ""
echo -e "${GREEN}Docker images built successfully${NC}"
echo ""
echo -e "${BLUE}Running integration tests in Docker container...${NC}"
echo ""

# Run the tests in a container
docker run --rm snow-owl-tftp-test

# Capture exit code
EXIT_CODE=$?

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo -e "${GREEN}All integration tests passed in Docker container!${NC}"
else
    echo -e "${RED}Tests failed with exit code: $EXIT_CODE${NC}"
fi

echo ""
echo -e "${BLUE}Running benchmark tests in Docker container...${NC}"
echo ""

# Run the tests in a container
docker run --rm snow-owl-tftp-benchmarks