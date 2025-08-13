#!/bin/bash
# Build script for Arrakis Arbitrage Service

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
IMAGE_NAME="${IMAGE_NAME:-arrakis-arbitrage}"
IMAGE_TAG="${IMAGE_TAG:-latest}"
REGISTRY="${REGISTRY:-}"

echo -e "${GREEN}🚀 Building Arrakis Arbitrage Service${NC}"
echo "Image: ${IMAGE_NAME}:${IMAGE_TAG}"

# Check if Docker is available
if ! command -v docker &> /dev/null; then
    echo -e "${RED}❌ Docker is not installed or not in PATH${NC}"
    exit 1
fi

# Build the Docker image
echo -e "${YELLOW}📦 Building Docker image...${NC}"
docker build \
    --tag "${IMAGE_NAME}:${IMAGE_TAG}" \
    --tag "${IMAGE_NAME}:latest" \
    --file Dockerfile \
    .

# Get image size
IMAGE_SIZE=$(docker images "${IMAGE_NAME}:${IMAGE_TAG}" --format "table {{.Size}}" | tail -n 1)
echo -e "${GREEN}✅ Build complete! Image size: ${IMAGE_SIZE}${NC}"

# Optional: Tag for registry if provided
if [ -n "${REGISTRY}" ]; then
    FULL_IMAGE="${REGISTRY}/${IMAGE_NAME}:${IMAGE_TAG}"
    echo -e "${YELLOW}🏷️  Tagging for registry: ${FULL_IMAGE}${NC}"
    docker tag "${IMAGE_NAME}:${IMAGE_TAG}" "${FULL_IMAGE}"
    
    echo -e "${GREEN}📤 To push to registry, run:${NC}"
    echo "docker push ${FULL_IMAGE}"
fi

echo -e "${GREEN}🎉 Build process completed successfully!${NC}"
echo -e "${YELLOW}💡 To run locally:${NC}"
echo "docker run -p 8000:8000 ${IMAGE_NAME}:${IMAGE_TAG}"
echo -e "${YELLOW}💡 Or use docker-compose:${NC}"
echo "docker-compose up"