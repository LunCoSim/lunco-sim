# Lunar Base Monitoring System - Deployment Guide

## Overview
This guide covers deploying the Lunar Base Monitoring System to Render using PostgreSQL.

## Prerequisites
- Node.js 18+
- Render account
- Render CLI installed: `npm install -g @render/cli`
- Git repository

## Quick Deploy

### 1. Setup Environment
```bash
# Clone repository
git clone <your-repo-url>
cd lunar-base-monitoring

# Install dependencies
npm install

# Copy environment template
cp .env.production.template .env.production
```

### 2. Configure Environment Variables
Edit `.env.production` with your values:
- `DATABASE_URL` - Will be auto-filled by Render
- `NEXTAUTH_URL` - Your Render app URL
- `NEXTAUTH_SECRET` - Generate a secure secret

### 3. Deploy to Render
```bash
# Login to Render
render login

# Deploy
./scripts/deploy.sh
```

## Manual Deployment Steps

### 1. Create Database
1. Go to Render Dashboard → New → PostgreSQL
2. Name: `lunar-base-db`
3. Database Name: `lunar_base_monitoring`
4. Choose plan (Starter recommended)
5. Create

### 2. Create Web Service
1. Go to Render Dashboard → New → Web Service
2. Connect your Git repository
3. Configure:
   - **Build Command**: `npm run build`
   - **Start Command**: `npm start`
   - **Health Check Path**: `/api/health`

### 3. Environment Variables
Set these in your web service:
```
NODE_ENV=production
DATABASE_URL=<from-database-settings>
NEXTAUTH_URL=<your-app-url>
NEXTAUTH_SECRET=<generate-secret>
APP_NAME=Lunar Base Monitoring System
LOG_LEVEL=info
ENABLE_DEV_TOOLS=false
MOCK_DATA=false
RATE_LIMIT_MAX=100
RATE_LIMIT_WINDOW=900000
ENABLE_METRICS=true
```

### 4. Database Setup
After deployment, the database will be automatically created via Prisma.

## Configuration Files

### render.yaml
Defines services and environment variables for automatic deployment.

### .env.production.template
Template for production environment variables.

### scripts/deploy.sh
Automated deployment script with validation.

## Post-Deployment Verification

1. **Health Check**: Visit `/api/health`
2. **Database Test**: Check if tables are created
3. **API Endpoints**: Test all API routes
4. **Frontend**: Verify dashboard loads correctly

## Monitoring

- **Logs**: Available in Render dashboard
- **Metrics**: Enabled via `ENABLE_METRICS=true`
- **Health Checks**: Automatic every 30 seconds

## Troubleshooting

### Database Connection Issues
- Verify `DATABASE_URL` is correct
- Check database is running
- Ensure Prisma schema matches

### Build Failures
- Check build logs in Render
- Verify all dependencies installed
- Ensure `NODE_VERSION` is compatible

### Runtime Errors
- Check application logs
- Verify environment variables
- Test API endpoints individually

## Scaling

### Web Service
- Upgrade plan for more CPU/memory
- Add instances for load balancing

### Database
- Upgrade plan for better performance
- Enable connection pooling
- Set up read replicas if needed

## Security

- Change `NEXTAUTH_SECRET` in production
- Use HTTPS (automatic on Render)
- Set appropriate CORS origins
- Enable rate limiting

## Support

For issues:
1. Check Render logs
2. Review this guide
3. Create GitHub issue
4. Contact Render support

---

**Deployment Time**: ~5-10 minutes
**Cost**: Starts at $7/month (Starter plan)
**Region**: Choose closest to your users