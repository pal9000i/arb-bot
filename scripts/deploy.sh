#!/bin/bash
# Kubernetes deployment script for Arrakis Arbitrage Service

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
NAMESPACE="${NAMESPACE:-arrakis-arbitrage}"
KUBECTL_CONTEXT="${KUBECTL_CONTEXT:-}"
DRY_RUN="${DRY_RUN:-false}"

echo -e "${GREEN}🚀 Deploying Arrakis Arbitrage Service to Kubernetes${NC}"

# Check if kubectl is available
if ! command -v kubectl &> /dev/null; then
    echo -e "${RED}❌ kubectl is not installed or not in PATH${NC}"
    exit 1
fi

# Set kubectl context if provided
if [ -n "${KUBECTL_CONTEXT}" ]; then
    echo -e "${YELLOW}🔄 Switching kubectl context to: ${KUBECTL_CONTEXT}${NC}"
    kubectl config use-context "${KUBECTL_CONTEXT}"
fi

# Show current context
CURRENT_CONTEXT=$(kubectl config current-context)
echo -e "${BLUE}📍 Current kubectl context: ${CURRENT_CONTEXT}${NC}"

# Validate deployment manifest
echo -e "${YELLOW}✅ Validating Kubernetes manifest...${NC}"
if ! kubectl apply --dry-run=client -f deployment.yaml; then
    echo -e "${RED}❌ Kubernetes manifest validation failed${NC}"
    exit 1
fi
echo -e "${GREEN}✅ Manifest validation passed${NC}"

# Deploy based on dry-run flag
if [ "${DRY_RUN}" = "true" ]; then
    echo -e "${YELLOW}🧪 Dry run mode - showing what would be deployed:${NC}"
    kubectl apply --dry-run=server -f deployment.yaml
    echo -e "${BLUE}💡 To deploy for real, run: DRY_RUN=false ./scripts/deploy.sh${NC}"
    exit 0
fi

# Create namespace if it doesn't exist
echo -e "${YELLOW}📁 Ensuring namespace exists: ${NAMESPACE}${NC}"
kubectl create namespace "${NAMESPACE}" --dry-run=client -o yaml | kubectl apply -f -

# Apply the deployment
echo -e "${YELLOW}📦 Applying Kubernetes resources...${NC}"
kubectl apply -f deployment.yaml

# Wait for deployment to be ready
echo -e "${YELLOW}⏳ Waiting for deployment to be ready...${NC}"
kubectl wait --for=condition=available --timeout=300s deployment/arrakis-deployment -n "${NAMESPACE}"

# Show deployment status
echo -e "${GREEN}✅ Deployment completed successfully!${NC}"
echo ""
echo -e "${BLUE}📊 Deployment Status:${NC}"
kubectl get pods,services,deployments -n "${NAMESPACE}" -o wide

echo ""
echo -e "${BLUE}🔍 Pod logs (last 20 lines):${NC}"
kubectl logs -n "${NAMESPACE}" -l app.kubernetes.io/name=arrakis-arbitrage --tail=20

echo ""
echo -e "${GREEN}🎉 Arrakis Arbitrage Service is now running!${NC}"
echo -e "${YELLOW}💡 Useful commands:${NC}"
echo "  View pods: kubectl get pods -n ${NAMESPACE}"
echo "  View logs: kubectl logs -n ${NAMESPACE} -l app.kubernetes.io/name=arrakis-arbitrage -f"
echo "  Scale deployment: kubectl scale deployment arrakis-deployment --replicas=5 -n ${NAMESPACE}"
echo "  Port forward for testing: kubectl port-forward -n ${NAMESPACE} service/arrakis-service 8000:8000"