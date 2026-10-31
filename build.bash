#!/bin/bash

# f0ckm build script - compatible version
# Targets: master, dev

# Detect docker compose command
if docker compose version >/dev/null 2>&1; then
    DOCKER_COMPOSE="docker compose"
elif docker-compose version >/dev/null 2>&1; then
    DOCKER_COMPOSE="docker-compose"
else
    echo "Error: Neither 'docker compose' nor 'docker-compose' found"
    exit 1
fi

# Parse arguments
TARGET="master"
CACHE_FLAG=""
CUSTOM_BRANCH=""

for arg in "$@"; do
    case $arg in
        master|dev|stg)
            TARGET=$arg
            ;;
        --no-cache)
            CACHE_FLAG="--no-cache"
            ;;
        *)
            if [ -z "$CUSTOM_BRANCH" ]; then
                CUSTOM_BRANCH=$arg
            fi
            ;;
    esac
done

# Check if we are in a git repo
if ! git rev-parse --is-inside-work-tree > /dev/null 2>&1; then
    echo "Error: not a git repository"
    exit 1
fi

# Set Image name and branch defaults
IMAGE_NAME="f0ckm"

if [ -n "$CUSTOM_BRANCH" ]; then
    BRANCH_TARGET="$CUSTOM_BRANCH"
    echo "--- CUSTOM BRANCH DEPLOYMENT (${BRANCH_TARGET}) ---"
elif [ "$TARGET" == "master" ]; then
    echo "--- PRODUCTION DEPLOYMENT (master) ---"
    BRANCH_TARGET="master"
elif [ "$TARGET" == "stg" ]; then
    echo "--- STAGING DEPLOYMENT ---"
    BRANCH_TARGET="dev"
else
    echo "--- LOCAL DEVELOPMENT (dev) ---"
    BRANCH_TARGET="dev"
fi

# Checkout branch
echo "Ensuring branch: ${BRANCH_TARGET}"
git checkout "${BRANCH_TARGET}" 2>/dev/null

# Pull latest changes
echo "Pulling latest changes for branch: $(git rev-parse --abbrev-ref HEAD)"
git pull

# Get git short revision
REV=$(git rev-parse --short HEAD)
TAG="f0ckm-${TARGET}-${REV}"

echo "Building image: ${IMAGE_NAME}:${TAG} for environment: ${TARGET} (Cache: ${CACHE_FLAG:-enabled})"

# Build the image (BuildKit: the Dockerfile caches cargo downloads and build output between builds)
DOCKER_BUILDKIT=1 docker build $CACHE_FLAG --build-arg GIT_HASH=${TAG} -f Dockerfile -t ${IMAGE_NAME}:${TAG} .

# Also tag it as latest
echo "Tagging as ${IMAGE_NAME}:latest"
docker tag ${IMAGE_NAME}:${TAG} ${IMAGE_NAME}:latest

# Update .env
if grep -q "^F0CKM_TAG=" .env; then
    sed -i "s/^F0CKM_TAG=.*/F0CKM_TAG=${TAG}/" .env
else
    [ -s .env ] && [ -n "$(tail -c 1 .env)" ] && echo "" >> .env
    echo "F0CKM_TAG=${TAG}" >> .env
fi

if grep -q "^F0CKM_IMAGE=" .env; then
    sed -i "s|^F0CKM_IMAGE=.*|F0CKM_IMAGE=${IMAGE_NAME}|" .env
else
    [ -s .env ] && [ -n "$(tail -c 1 .env)" ] && echo "" >> .env
    echo "F0CKM_IMAGE=${IMAGE_NAME}" >> .env
fi

# Deployment
echo "Restarting services with $DOCKER_COMPOSE..."
$DOCKER_COMPOSE down
$DOCKER_COMPOSE up -d

# Cleanup
# if you want to clear up all the old images uncomment
# otherwise to prevent clutter clean it from time to time
#echo "Cleaning up old docker resources..."
#docker system prune -af

echo "Successfully updated ${TARGET} with tag ${TAG}"
