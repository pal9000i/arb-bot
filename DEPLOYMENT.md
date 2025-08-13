# Deployment Guide

This guide covers deploying the Arrakis Arbitrage Service to production using Docker and Kubernetes.

## Prerequisites

- Docker (for building and testing)
- Kubernetes cluster access
- kubectl configured
- Valid RPC endpoints for Ethereum and Base
- Deployed Uniswap V4 State View contract

## Quick Start

### 1. Environment Setup

```bash
# Copy and configure environment variables
cp .env.example .env
# Edit .env with your actual values
```

### 2. Build Docker Image

```bash
# Using the build script
./scripts/build.sh

# Or manually
docker build -t arrakis-arbitrage:latest .
```

### 3. Test Locally

```bash
# Using docker-compose
docker-compose up

# Or run container directly
docker run -p 8000:8000 --env-file .env arrakis-arbitrage:latest
```

### 4. Deploy to Kubernetes

```bash
# Update secrets in deployment.yaml with your actual values
# Then deploy
./scripts/deploy.sh

# Or manually
kubectl apply -f deployment.yaml
```

## Configuration

### Required Environment Variables

| Variable | Description | Example |
|----------|-------------|---------|
| `ETHEREUM_RPC_URL` | Ethereum mainnet RPC endpoint | `https://eth-mainnet.g.alchemy.com/v2/API_KEY` |
| `BASE_RPC_URL` | Base mainnet RPC endpoint | `https://base-mainnet.g.alchemy.com/v2/API_KEY` |
| `UNISWAP_V4_STATE_VIEW` | Deployed StateView contract address | `0x...` |

### Optional Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `info` | Log level |
| `ROCKET_PORT` | `8000` | Service port |
| `ACROSS_TIMEOUT_SECS` | `10` | API timeout |

## Production Deployment

### Docker Image Registry

```bash
# Tag for your registry
docker tag arrakis-arbitrage:latest your-registry.com/arrakis-arbitrage:latest

# Push to registry
docker push your-registry.com/arrakis-arbitrage:latest
```

### Kubernetes Deployment

1. **Update Secrets**: Edit `deployment.yaml` to include your actual RPC URLs and contract addresses
2. **Apply Resources**: `kubectl apply -f deployment.yaml`
3. **Verify Deployment**: `kubectl get pods -n arrakis-arbitrage`

### Monitoring (Optional)

Deploy monitoring resources for observability:

```bash
kubectl apply -f monitoring.yaml
```

This includes:
- ServiceMonitor for Prometheus
- Grafana dashboard
- Alert rules
- Metrics service

## Security Considerations

### Container Security
- Runs as non-root user (UID 1000)
- Read-only root filesystem
- Minimal runtime dependencies
- No privileged capabilities

### Kubernetes Security
- NetworkPolicy for traffic isolation
- RBAC with minimal permissions
- PodSecurityPolicy compliance
- Secret management for sensitive data

### Network Security
- TLS for all external communications
- Firewall rules for egress traffic
- VPC isolation recommended

## Scaling

### Horizontal Scaling
```bash
kubectl scale deployment arrakis-deployment --replicas=5 -n arrakis-arbitrage
```

### Resource Limits
Current limits per pod:
- CPU: 1000m (1 core)
- Memory: 512Mi
- Requests: 100m CPU, 128Mi memory

### High Availability
- 3 replica minimum
- PodDisruptionBudget ensures 2 pods always available
- Pod anti-affinity spreads across nodes

## Health Checks

### Application Health
- HTTP health endpoint: `GET /health`
- Kubernetes liveness probe: 60s initial delay, 30s interval
- Kubernetes readiness probe: 10s initial delay, 10s interval

### Monitoring Endpoints
- Metrics: `GET /metrics` (Prometheus format)
- API endpoints: `GET /api/v1/arbitrage-opportunity`

## Troubleshooting

### Common Issues

1. **Pod Not Starting**
   ```bash
   kubectl describe pod <pod-name> -n arrakis-arbitrage
   kubectl logs <pod-name> -n arrakis-arbitrage
   ```

2. **Connection Issues**
   - Verify RPC URLs are accessible
   - Check firewall/network policies
   - Validate contract addresses

3. **High Memory Usage**
   - Monitor with `kubectl top pods -n arrakis-arbitrage`
   - Adjust resource limits if needed
   - Check for memory leaks in logs

### Debug Commands

```bash
# View pod status
kubectl get pods -n arrakis-arbitrage -o wide

# Check logs
kubectl logs -n arrakis-arbitrage -l app.kubernetes.io/name=arrakis-arbitrage -f

# Port forward for testing
kubectl port-forward -n arrakis-arbitrage service/arrakis-service 8000:8000

# Execute into container
kubectl exec -it <pod-name> -n arrakis-arbitrage -- /bin/bash
```

## Performance Tuning

### CPU Optimization
- Rust release build with optimizations
- Multi-stage Docker build reduces image size
- Async runtime for concurrent operations

### Memory Optimization
- Efficient BigInt arithmetic
- Connection pooling for RPC clients
- Bounded channels for backpressure

### Network Optimization
- Keep-alive connections
- Request timeout configuration
- Circuit breaker patterns (implement as needed)

## Backup and Recovery

### State Management
- Service is stateless
- No persistent data storage required
- Configuration via environment variables

### Disaster Recovery
- Multi-region deployment recommended
- Database backup not required (stateless)
- Blue-green deployment for zero-downtime updates

## Updates and Maintenance

### Rolling Updates
```bash
# Update image
kubectl set image deployment/arrakis-deployment arrakis-arbitrage=arrakis-arbitrage:new-version -n arrakis-arbitrage

# Monitor rollout
kubectl rollout status deployment/arrakis-deployment -n arrakis-arbitrage

# Rollback if needed
kubectl rollout undo deployment/arrakis-deployment -n arrakis-arbitrage
```

### Maintenance Windows
- Service designed for 24/7 operation
- Zero-downtime updates supported
- Health checks ensure traffic routing

## Cost Optimization

### Resource Efficiency
- Right-size CPU/memory requests and limits
- Use spot instances where appropriate
- Scale down during low-traffic periods

### RPC Cost Management
- Implement request caching where possible
- Use rate limiting to prevent abuse
- Monitor RPC usage and costs

## Compliance and Auditing

### Security Scanning
- Regular container image scanning
- Dependency vulnerability checks
- SAST/DAST in CI/CD pipeline

### Logging and Auditing
- Structured logging with correlation IDs
- Audit trails for all transactions
- Log retention policy implementation

### Regulatory Compliance
- Data privacy considerations
- Geographic restrictions if applicable
- Financial services compliance as needed