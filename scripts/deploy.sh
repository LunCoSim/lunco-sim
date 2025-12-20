#!/bin/bash

# Lunar Base Monitoring System - Deployment Script
# This script helps deploy the application to Render

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
APP_NAME="lunar-base-monitoring"
RENDER_URL="https://dashboard.render.com"

echo -e "${GREEN}🚀 Lunar Base Monitoring System Deployment${NC}"
echo "=================================="

# Check if Render CLI is installed
if ! command -v render &> /dev/null; then
    echo -e "${YELLOW}⚠️  Render CLI not found. Please install it first:${NC}"
    echo "npm install -g @render/cli"
    echo "Then run: render login"
    exit 1
fi

# Check if user is logged in to Render
if ! render whoami &> /dev/null; then
    echo -e "${YELLOW}⚠️  Not logged in to Render. Please run:${NC}"
    echo "render login"
    exit 1
fi

echo -e "${GREEN}✅ Render CLI authenticated${NC}"

# Validate environment
echo -e "${YELLOW}🔍 Validating environment...${NC}"

# Check if .env.production exists
if [ ! -f ".env.production" ]; then
    echo -e "${YELLOW}⚠️  .env.production not found. Creating from template...${NC}"
    if [ -f ".env.production.template" ]; then
        cp .env.production.template .env.production
        echo -e "${GREEN}✅ Created .env.production from template${NC}"
        echo -e "${YELLOW}⚠️  Please edit .env.production with your values before deploying${NC}"
    else
        echo -e "${RED}❌ .env.production.template not found${NC}"
        exit 1
    fi
fi

# Check if render.yaml exists
if [ ! -f "render.yaml" ]; then
    echo -e "${RED}❌ render.yaml not found${NC}"
    exit 1
fi

# Run database migration
echo -e "${YELLOW}🗄️  Running database migrations...${NC}"
npm run db:push

# Build the application
echo -e "${YELLOW}🔨 Building application...${NC}"
npm run build

# Deploy to Render
echo -e "${YELLOW}🚀 Deploying to Render...${NC}"
if render deploy --confirm; then
    echo -e "${GREEN}✅ Deployment successful!${NC}"
    echo -e "${GREEN}🌐 Your app is available at: ${RENDER_URL}${NC}"
else
    echo -e "${RED}❌ Deployment failed${NC}"
    exit 1
fi

# Post-deployment checks
echo -e "${YELLOW}🔍 Running post-deployment checks...${NC}"

# Wait a bit for the app to start
sleep 30

# Check health endpoint
if [ -n "$RENDER_APP_URL" ]; then
    if curl -f "$RENDER_APP_URL/api/health" > /dev/null 2>&1; then
        echo -e "${GREEN}✅ Health check passed${NC}"
    else
        echo -e "${YELLOW}⚠️  Health check failed, but deployment may still be starting${NC}"
    fi
fi

echo -e "${GREEN}🎉 Deployment completed!${NC}"
echo "=================================="
echo "Next steps:"
echo "1. Visit your app on Render dashboard"
echo "2. Monitor the logs for any issues"
echo "3. Test all API endpoints"
echo "4. Verify database connectivity"