"""
Byzantine Agreement Protocol with Drift Detection (BAP-DD) - Prototype
Implements EWMC (Exponentially Weighted Moving Consensus) for Class C fault detection.
"""

import math
from typing import List, Dict

class BAPDD:
    def __init__(self, num_granules: int = 4, alpha: float = 0.95, beta: float = 10.0):
        self.num_granules = num_granules
        self.alpha = alpha
        self.beta = beta
        # Drift vectors D_ij for all i, j
        self.drift_vectors = [[0.0 for _ in range(num_granules)] for _ in range(num_granules)]
        # Weights for each granule
        self.weights = [1.0 for _ in range(num_granules)]

    def update_drift(self, opinions: List[float]):
        """Updates drift vectors based on current opinions (e.g., heart rate estimates)."""
        for i in range(self.num_granules):
            for j in range(self.num_granules):
                # Using simple L2 norm for scalar opinions
                diff = abs(opinions[i] - opinions[j])
                self.drift_vectors[i][j] = (self.alpha * self.drift_vectors[i][j] +
                                            (1 - self.alpha) * diff)

        self._update_weights()

    def _update_weights(self):
        """Recalculates authority weights based on accumulated drift."""
        for j in range(self.num_granules):
            sum_drift = sum(self.drift_vectors[i][j] for i in range(self.num_granules) if i != j)
            self.weights[j] = 1.0 / (1.0 + self.beta * sum_drift)

    def get_consensus_opinion(self, opinions: List[float]) -> float:
        """Calculates the weighted median consensus opinion."""
        # Simple weighted average for the prototype
        total_weight = sum(self.weights)
        if total_weight == 0:
            return sum(opinions) / len(opinions)

        weighted_sum = sum(o * w for o, w in zip(opinions, self.weights))
        return weighted_sum / total_weight

def main():
    bap = BAPDD(num_granules=4)

    # Simulate 100 rounds of consensus
    print("Starting simulation (Granule 1 is drifting)...")
    for t in range(100):
        # Normal granules α, γ, δ report ~70 BPM
        # Granule β (index 1) has a 10% systematic bias (Class C)
        opinions = [70.0, 77.0, 70.0, 70.0]
        bap.update_drift(opinions)

        if t % 20 == 0:
            print(f"Round {t}: Weights = {[round(w, 3) for w in bap.weights]}")
            print(f"         Consensus Opinion = {round(bap.get_consensus_opinion(opinions), 2)}")

    print("\nFinal State:")
    print(f"Final Weights: {[round(w, 3) for w in bap.weights]}")
    # Granule 1 should have significantly lower weight

if __name__ == "__main__":
    main()
