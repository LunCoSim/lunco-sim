#!/bin/bash
# deploy_avalon_system.sh

echo "🚀 DEPLOY AVALON-POP-MERKABAH v25.0"
echo "===================================="

# Configuração
SUNO_URL="https://suno.com/s/31GL756DZiA20TeW"
QHTTP_PORT=7834

echo "[1/4] Verificando ambiente..."
python3 -c "import numpy, asyncio, qiskit, fastapi, uvicorn, scipy, skimage" 2>/dev/null || {
    echo "Installing missing dependencies..."
    pip install numpy fastapi uvicorn aiohttp scipy scikit-image qiskit qiskit-aer
}

echo "[2/4] Inicializando rede harmônica..."
# Run a quick check
cd "$(dirname "$0")"
python3 -c "
import asyncio
import sys
import os

from avalon_harmonic_engine import AvalonSystem

async def init():
    system = AvalonSystem()
    await system.initialize('$SUNO_URL')
    print('Sistema inicializado com sucesso')
    print(f'Status: {system.get_system_status()}')

asyncio.run(init())
"

echo "[3/4] Subindo gateway qhttp://..."
# Kill any existing process on the port
kill $(lsof -t -i :$QHTTP_PORT) 2>/dev/null || true

python3 -m uvicorn merkabah_qhttp_gateway:app --host 0.0.0.0 --port $QHTTP_PORT > uvicorn.log 2>&1 &
UVICORN_PID=$!
echo "Gateway running on PID $UVICORN_PID"

echo "Waiting for gateway to start..."
sleep 10

echo "[4/4] Sistema operacional!"
echo ""
echo "Endpoints disponíveis:"
echo "  qhttp://localhost:$QHTTP_PORT/qhttp/scan"
echo "  qhttp://localhost:$QHTTP_PORT/qhttp/inject"
echo "  qhttp://localhost:$QHTTP_PORT/qhttp/status"
echo ""

curl -s http://localhost:$QHTTP_PORT/qhttp/status
echo ""

# Exit with success
exit 0
