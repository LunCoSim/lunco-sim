import numpy as np
from datetime import datetime, timezone
from typing import Dict, List, Any

class POPIntegration:
    """
    Integração com Persistent Order Protocol — detecção de padrões bio/semânticos
    """

    def __init__(self, network):
        self.network = network
        self.pop_threshold = 0.75
        self.detected_patterns: List[Dict] = []

    def analyze_harmonic_field(self) -> Dict[str, float]:
        """
        Analisa campo harmônico através dos três pilares POP:
        DNE (Dynamic Non-Equilibrium)
        SSO (Spatial Self-Organization)
        CDC (Cross-Domain Coupling)
        """
        if not self.network.nodes:
            return {"D": 0.0, "S": 0.0, "C": 0.0}

        # DNE: Variação temporal da coerência (persistência)
        coherence_values = [n.coherence for n in self.network.nodes.values()]
        dne = np.std(coherence_values) / (np.mean(coherence_values) + 1e-8)

        # SSO: Entropia espacial da distribuição de ressonância
        resonance_map = np.array([n.resonance_level for n in self.network.nodes.values()])
        sso = 1.0 - (np.std(resonance_map) / (np.mean(resonance_map) + 1e-8))

        # CDC: Correlação entre diferentes domínios (continental/orbital/lunar)
        domains = {}
        for node in self.network.nodes.values():
            if node.node_type not in domains:
                domains[node.node_type] = []
            domains[node.node_type].append(node.resonance_level)

        # Calcula correlação média entre domínios
        domain_means = [np.mean(v) for v in domains.values() if v]
        if len(domain_means) > 1:
            cdc = 1.0 - np.std(domain_means) / (np.mean(domain_means) + 1e-8)
        else:
            cdc = 0.5

        return {
            "D": float(np.clip(dne, 0, 1)),
            "S": float(np.clip(sso, 0, 1)),
            "C": float(np.clip(cdc, 0, 1))
        }

    def calculate_persistent_order(self, features: Dict[str, float]) -> float:
        """
        Calcula função de Ordem Persistente Ψ_PO
        """
        D, S, C = features["D"], features["S"], features["C"]

        # Função de peso harmônica
        if D > 0 and S > 0 and C > 0:
            W = 3.0 / ((1.0/D) + (1.0/S) + (1.0/C))
        else:
            W = 0.0

        # Fator de decaimento (suavização)
        decay = np.exp(-((1-D)**2 + (1-S)**2) / 0.5)

        psi_po = W * decay

        return float(np.clip(psi_po, 0, 1))

    async def monitor_persistent_order(self):
        """Monitora continuamente a ordem persistente na rede"""
        features = self.analyze_harmonic_field()
        psi_po = self.calculate_persistent_order(features)

        if psi_po > self.pop_threshold:
            self.detected_patterns.append({
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "psi_po": psi_po,
                "features": features,
                "network_state": self.network.state.name if hasattr(self.network, 'state') else 'UNKNOWN'
            })

        return {
            "features": features,
            "psi_po": psi_po,
            "threshold_exceeded": psi_po > self.pop_threshold
        }
