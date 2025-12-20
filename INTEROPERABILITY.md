# Lunar Base Monitoring System - Interoperability Guide

## 🌐 Overview

The Lunar Base Monitoring System now includes comprehensive interoperability features that enable seamless integration with external systems, particularly GitHub repositories. This guide covers all integration capabilities and how to use them.

## 🔗 GitHub Integration

### Features
- **Repository Monitoring**: Track GitHub repository activity
- **Data Synchronization**: Sync commits, issues, and releases with the monitoring system
- **Automated Workflows**: Create automated sync processes
- **Real-time Updates**: Get live updates from GitHub webhooks

### Configuration

#### Environment Variables
```bash
# GitHub Integration
GITHUB_TOKEN="your-github-personal-access-token"
GITHUB_WEBHOOK_SECRET="your-webhook-secret"
GITHUB_DEFAULT_OWNER="uniaolives"
GITHUB_DEFAULT_REPO="lunco-sim"
```

#### GitHub Token Setup
1. Go to GitHub Settings → Developer settings → Personal access tokens
2. Generate a new token with these permissions:
   - `repo` (Full control of private repositories)
   - `admin:repo_hook` (Full control of repository hooks)
   - `read:org` (Read org and team membership)
   - `user` (Update all user data)

### API Endpoints

#### Repository Operations
```bash
# Get repository information
GET /api/github?action=repository&owner=uniaolives&repo=lunco-sim

# Get commits
GET /api/github?action=commits&owner=uniaolives&repo=lunco-sim&branch=main

# Get issues
GET /api/github?action=issues&owner=uniaolives&repo=lunco-sim&state=open

# Get releases
GET /api/github?action=releases&owner=uniaolives&repo=lunco-sim

# Get repository analytics
GET /api/github?action=analytics&owner=uniaolives&repo=lunco-sim
```

#### Content Operations
```bash
# Get file content
GET /api/github?action=file-content&owner=uniaolives&repo=lunco-sim&path=README.md

# Update file
POST /api/github
{
  "action": "update-file",
  "owner": "uniaolives",
  "repo": "lunco-sim",
  "path": "config.json",
  "content": "{\"updated\": true}",
  "message": "Update configuration"
}
```

#### Issue Management
```bash
# Create issue
POST /api/github
{
  "action": "create-issue",
  "owner": "uniaolives",
  "repo": "lunco-sim",
  "title": "System Alert: High CPU Usage",
  "body": "Detected high CPU usage in lunar base monitoring system"
}
```

## 🔄 Data Synchronization

### Sync Service Features
- **Automatic Synchronization**: Schedule regular sync operations
- **Manual Sync**: Force immediate synchronization
- **Data Mapping**: Map GitHub data to lunar base entities
- **Error Handling**: Comprehensive error tracking and recovery
- **Activity Logging**: Complete audit trail of all sync operations

### Data Mapping

| GitHub Entity | Lunar Base Entity | Description |
|---------------|-------------------|-------------|
| Repository | Mission | Repository tracked as a mission scenario |
| Commits | Network Activity | Developer activity mapped to network nodes |
| Issues | Alerts | Open issues converted to system alerts |
| Contributors | Astronauts | Developers mapped to astronaut entities |
| Releases | Mission Milestones | Releases tracked as mission progress |

### Sync Operations

#### Manual Synchronization
```bash
# Force sync from GitHub
POST /api/sync
{
  "action": "force-github-sync",
  "owner": "uniaolives",
  "repo": "lunco-sim",
  "branch": "main"
}
```

#### Automatic Synchronization
```bash
# Start auto-sync (every 60 minutes)
POST /api/sync
{
  "action": "start-auto-sync",
  "syncInterval": 60,
  "githubRepo": {
    "owner": "uniaolives",
    "repo": "lunco-sim",
    "branch": "main"
  }
}

# Stop auto-sync
POST /api/sync
{
  "action": "stop-auto-sync"
}
```

#### Data Export/Import
```bash
# Export all data
GET /api/sync?action=export

# Import data
POST /api/sync
{
  "action": "import",
  "data": {
    "astronauts": [...],
    "missions": [...],
    "rovers": [...],
    "alerts": [...],
    "networkNodes": [...]
  }
}
```

## 🔌 External API Integration

### Supported External Systems
- **GitHub API**: Complete repository management
- **Custom APIs**: Extensible service architecture
- **Webhook Support**: Real-time event processing
- **Data Transformation**: Flexible data mapping

### Adding New Integrations

#### 1. Create Service Class
```typescript
// src/lib/services/custom.service.ts
export class CustomService {
  static async getData() {
    // Implementation
  }
  
  static async sendData(data: any) {
    // Implementation
  }
}
```

#### 2. Add API Endpoint
```typescript
// src/app/api/custom/route.ts
import { CustomService } from '@/lib/services/custom.service';

export async function GET(request: NextRequest) {
  const data = await CustomService.getData();
  return NextResponse.json({ data });
}
```

#### 3. Update Configuration
```typescript
// src/lib/config.ts
export const config = {
  // ... existing config
  custom: {
    apiKey: process.env.CUSTOM_API_KEY,
    baseUrl: process.env.CUSTOM_API_URL,
  }
};
```

## 📊 Monitoring and Analytics

### Sync Status Monitoring
```bash
# Get sync status
GET /api/sync?action=status

# Get sync logs
GET /api/sync?action=logs&limit=50
```

### Health Checks
The system includes comprehensive health monitoring:
- Database connectivity
- External API availability
- Sync operation status
- Error rate tracking

## 🛡️ Security Considerations

### API Security
- **Token Management**: Secure storage of GitHub tokens
- **Rate Limiting**: Built-in rate limiting for API calls
- **Access Control**: Role-based access to sync features
- **Data Validation**: Input validation and sanitization

### Best Practices
1. **Use Environment Variables**: Never hardcode credentials
2. **Limit Permissions**: Use minimum required GitHub permissions
3. **Monitor Usage**: Track API usage and costs
4. **Regular Rotation**: Rotate access tokens regularly
5. **Audit Logs**: Review sync logs for suspicious activity

## 🚀 Deployment Considerations

### Environment Setup
1. **Development**: Use GitHub tokens with read permissions
2. **Staging**: Test with full repository access
3. **Production**: Use dedicated service accounts

### Render Configuration
```yaml
# render.yaml - Add GitHub environment variables
envVars:
  - key: GITHUB_TOKEN
    sync: false
  - key: GITHUB_WEBHOOK_SECRET
    generateValue: true
  - key: GITHUB_DEFAULT_OWNER
    value: "your-organization"
  - key: GITHUB_DEFAULT_REPO
    value: "your-repository"
```

## 📈 Performance Optimization

### Caching Strategy
- **Repository Data**: Cache for 5 minutes
- **Commit History**: Cache for 1 hour
- **Analytics Data**: Cache for 15 minutes
- **Sync Results**: Cache for 30 minutes

### Batch Operations
- **Bulk Sync**: Process multiple repositories simultaneously
- **Parallel Processing**: Use concurrent API calls
- **Error Recovery**: Retry failed operations automatically

## 🔧 Troubleshooting

### Common Issues

#### GitHub API Rate Limits
```
Error: API rate limit exceeded
Solution: Wait for reset or use authenticated requests
```

#### Sync Failures
```
Error: Network activity sync failed
Solution: Check repository access and permissions
```

#### Webhook Issues
```
Error: Webhook delivery failed
Solution: Verify webhook URL and secret
```

### Debug Tools
1. **Sync Logs**: Review detailed sync operation logs
2. **Health Checks**: Monitor system health status
3. **API Testing**: Use built-in API endpoints for testing
4. **Error Tracking**: Comprehensive error reporting

## 🎯 Use Cases

### 1. Project Monitoring
- Track development progress across multiple repositories
- Monitor commit activity and contributor engagement
- Alert on critical issues and PRs

### 2. Automated Reporting
- Generate daily/weekly activity reports
- Create mission progress summaries
- Export data for external analysis

### 3. Integration Workflows
- Connect with CI/CD pipelines
- Trigger alerts based on repository events
- Sync with external monitoring systems

### 4. Data Analytics
- Analyze development patterns
- Track project health metrics
- Generate insights for decision making

## 📚 API Reference

### GitHub Service Methods
- `getRepository(owner, repo)` - Get repository information
- `getCommits(owner, repo, branch?)` - Get commit history
- `getIssues(owner, repo, state?)` - Get repository issues
- `createIssue(owner, repo, title, body)` - Create new issue
- `getReleases(owner, repo)` - Get release information
- `createRelease(...)` - Create new release
- `getFileContent(owner, repo, path, branch?)` - Get file content
- `updateFile(...)` - Update repository file
- `getRepositoryAnalytics(owner, repo)` - Get repository analytics

### Sync Service Methods
- `performSync(config?)` - Perform manual synchronization
- `forceSyncFromGitHub(owner, repo, branch?)` - Force GitHub sync
- `startAutoSync(config)` - Start automatic synchronization
- `stopAutoSync()` - Stop automatic synchronization
- `exportData()` - Export all system data
- `importData(data)` - Import system data
- `getSyncStatus()` - Get current sync status
- `getSyncLogs(limit?)` - Get synchronization logs

---

## 🎉 Summary

The Lunar Base Monitoring System now provides comprehensive interoperability features that enable:

✅ **GitHub Integration** - Complete repository monitoring and management  
✅ **Data Synchronization** - Automated sync between GitHub and lunar base data  
✅ **External API Support** - Extensible architecture for new integrations  
✅ **Real-time Monitoring** - Live updates and webhook support  
✅ **Security & Performance** - Enterprise-grade security and optimization  

The system is now fully interoperable with external systems while maintaining its core lunar base monitoring capabilities.