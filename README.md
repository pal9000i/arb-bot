# Arrakis Finance Arbitrage Monitoring Service - Developer Guide

A production-ready Rust service that monitors arbitrage opportunities between Uniswap V4 (Ethereum) and Aerodrome Finance (Base) for WETH/USDC pairs.

## 🚀 Quick Start

```bash
# 1. Clone and setup
git clone <repository-url>
cd arrakis-arbitrage

# 2. Configure environment
cp secrets.env.example secrets.env
# Edit secrets.env with your RPC URLs and API keys

# 3. Run locally
cargo run

# 4. Test the API
curl "http://localhost:8000/api/v1/arbitrage-opportunity?trade_size_eth=10"
```

## 📋 Prerequisites

- **Rust** (latest stable) - [Install here](https://rustup.rs/)
- **Docker** (for containerization)
- **kubectl** + **minikube** (for Kubernetes testing)
- **RPC Access**: Ethereum and Base RPC endpoints (e.g., Alchemy, Infura)

## 🔧 Environment Configuration

### Required Environment Variables

Configuration is managed through two files:
- `addresses.env` - Public contract addresses and protocol configuration
- `secrets.env` - Sensitive RPC URLs and API keys (⚠️ never commit this file)

### Creating secrets.env

Copy the example file and add your RPC endpoints:

```bash
# Copy example file
cp secrets.env.example secrets.env

# Edit secrets.env with your actual API keys
# Replace YOUR_ETHEREUM_KEY and YOUR_BASE_KEY with real values
```

### Getting API Keys

1. **Alchemy** (Recommended):
   - Sign up at [alchemy.com](https://alchemy.com)
   - Create apps for "Ethereum Mainnet" and "Base Mainnet"
   - Copy the HTTP URLs to `ETHEREUM_RPC_URL` and `BASE_RPC_URL` in `secrets.env`

2. **Alternative RPC Providers**:
   - Infura, QuickNode, Ankr, or any Ethereum/Base RPC provider
   - Update the URLs in `secrets.env` accordingly

## 🏗️ Local Development

### Setup

```bash
# Install Rust if not already installed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone repository
git clone <repository-url>
cd arrakis-arbitrage

# Copy and configure environment
cp secrets.env.example secrets.env
# Edit secrets.env with your RPC URLs
```

### Running Locally

```bash
# Run the service
cargo run

# The service starts on http://localhost:8000
# API endpoints:
# GET /health                 - Health check
# GET /metrics               - Prometheus metrics  
# GET /api/v1/arbitrage-opportunity?trade_size_eth=<amount>
```

### Testing

```bash
# Run all tests
cargo test

# Run specific test suites
cargo test --test arbitrage_optimizer_integration
cargo test --test api_integration_test
cargo test --test blockchain_tests

# Test API manually
curl "http://localhost:8000/api/v1/arbitrage-opportunity?trade_size_eth=5"
```

## 🐳 Docker Deployment

### Build Docker Image

```bash
# Build the image (uses pinned Rust 1.89.0 for reproducible builds)
docker build -t arrakis-arbitrage:latest .

# Verify the build
docker images | grep arrakis-arbitrage
```

### Run with Docker

```bash
# Run container (loads both addresses.env and secrets.env)
docker run --env-file addresses.env --env-file secrets.env -p 8000:8000 arrakis-arbitrage:latest

# Test the containerized service
curl "http://localhost:8000/health"
curl "http://localhost:8000/api/v1/arbitrage-opportunity?trade_size_eth=1"
```

### Run with Docker Compose

Docker Compose automatically loads both configuration files:

```bash
# Start the service stack (automatically uses secrets.env and addresses.env)
docker-compose up -d

# View logs
docker-compose logs -f

# Stop the service
docker-compose down
```

## ☸️ Kubernetes Deployment

### Production Kubernetes

```bash
# Create namespace
kubectl create namespace arrakis-arbitrage

# Create ConfigMap for public addresses
kubectl create configmap arrakis-addresses \
  --from-env-file=addresses.env \
  -n arrakis-arbitrage

# Create Secret for sensitive RPC URLs
kubectl create secret generic arrakis-secrets \
  --from-env-file=secrets.env \
  -n arrakis-arbitrage

# Deploy production manifests
kubectl apply -f deployment.yaml

# Optional: Deploy monitoring
kubectl apply -f monitoring.yaml
```

## 📊 API Reference

### Endpoints

#### GET `/health`
Health check endpoint.

**Response:**
```json
"OK"
```

#### GET `/metrics`
Prometheus metrics endpoint.

**Response:**
```
# TYPE arrakis_uptime_seconds counter
arrakis_uptime_seconds 1
# TYPE arrakis_requests_total counter
arrakis_requests_total{endpoint="health"} 1
```

#### GET `/api/v1/arbitrage-opportunity`
Main arbitrage analysis endpoint.

**Parameters:**
- `trade_size_eth` (required): Trade size in ETH (e.g., `10.0`)

**Example Request:**
```bash
curl "http://localhost:8000/api/v1/arbitrage-opportunity?trade_size_eth=5"
```

**Example Response:**
```json
{
  "timestamp_utc": "2025-08-13T21:34:36.828941+00:00",
  "trade_size_eth": 5.0,
  "reference_cex_price_usd": 4766.645,
  "uniswap_v4_details": {
    "sell_price_usdc_per_eth": 4739.43,
    "buy_price_usdc_per_eth": 4777.31,
    "price_impact_percent": -0.097,
    "estimated_gas_cost_usd": 0.96
  },
  "aerodrome_details": {
    "sell_price_usdc_per_eth": 4741.87,
    "buy_price_usdc_per_eth": 4792.54,
    "price_impact_percent": -0.529,
    "estimated_gas_cost_usd": 0.008
  },
  "arbitrage_summary": {
    "spread_uni_to_aero": -53.11,
    "spread_aero_to_uni": -35.44,
    "gross_profit_uni_to_aero_usd": -265.57,
    "gross_profit_aero_to_uni_usd": -177.18,
    "total_gas_cost_usd": 2.79,
    "bridge_cost_usd": 1.82,
    "net_profit_best_usd": 0.0,
    "recommended_action": "NO_ARBITRAGE"
  }
}
```

### Response Fields

- **timestamp_utc**: When the analysis was performed
- **trade_size_eth**: Requested trade size in ETH
- **reference_cex_price_usd**: Current ETH price from CEX
- **uniswap_v4_details**: Uniswap V4 pricing and gas costs
- **aerodrome_details**: Aerodrome pricing and gas costs  
- **arbitrage_summary**: Profitability analysis
- **recommended_action**: `ARBITRAGE_UNI_TO_AERO`, `ARBITRAGE_AERO_TO_UNI`, or `NO_ARBITRAGE`


### Logging

Set `RUST_LOG` environment variable for detailed logs:

```bash
# Debug level logging
RUST_LOG=debug cargo run

# Info level (default)
RUST_LOG=info cargo run

# Error level only
RUST_LOG=error cargo run
```


## 🚀 Production Considerations

### Security
- ✅ **secrets.env protection**: File is git-ignored and never committed
- ✅ **Kubernetes Secrets**: Sensitive RPC URLs stored as K8s secrets
- ✅ **Docker secrets**: Environment files loaded securely in containers
- ✅ **Network policies**: Restrict pod communication (configured)
- ✅ **Non-root containers**: Run as uid 1000 with read-only filesystem

### Monitoring
- Prometheus metrics exposed at `/metrics`
- Health checks at `/health` 
- Configure alerting on high error rates
- Monitor RPC provider rate limits

### Scaling
- Service is stateless and can be horizontally scaled
- Consider caching layer for frequently accessed data
- Use multiple RPC providers for redundancy