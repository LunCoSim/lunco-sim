"""
Fault Injection Model (FIM) v1.0 - Core Prototype
Implements basic SEU (Single Event Upset) injection logic for radiation-resilience testing.
"""

import random
import struct
from dataclasses import dataclass
from typing import List, Any

@dataclass
class SEUProfile:
    """Radiation environment simulation profile"""
    bit_flip_rate: float = 1e-6
    spatial_locality: float = 0.3
    energy_threshold: float = 0.5

class FIMCore:
    def __init__(self, profile: SEUProfile = SEUProfile()):
        self.profile = profile

    def inject_bit_flip(self, value: float) -> float:
        """Simulates a single bit flip in a 32-bit float."""
        # Convert float to 32-bit binary representation
        packed = struct.pack('!f', value)
        int_val = struct.unpack('!I', packed)[0]

        # Select a random bit to flip
        bit_pos = random.randint(0, 31)
        corrupted_int = int_val ^ (1 << bit_pos)

        # Convert back to float
        corrupted_packed = struct.pack('!I', corrupted_int)
        return struct.unpack('!f', corrupted_packed)[0]

    def inject_stuck_at(self, value: float, state: float = 0.0) -> float:
        """Simulates a stuck-at fault."""
        return state

    def inject_drift(self, value: float, intensity: float = 0.1) -> float:
        """Simulates a drift fault (gradual synaptic degradation)."""
        drift = random.gauss(0, intensity)
        return value * (1 + drift)

    def apply_faults(self, weights: List[float], fault_type: str = "bit_flip") -> List[float]:
        """Applies specified faults to a list of weights based on the SEU profile."""
        corrupted_weights = []
        for w in weights:
            if random.random() < self.profile.bit_flip_rate:
                if fault_type == "bit_flip":
                    corrupted_weights.append(self.inject_bit_flip(w))
                elif fault_type == "stuck_at":
                    corrupted_weights.append(self.inject_stuck_at(w))
                elif fault_type == "drift":
                    corrupted_weights.append(self.inject_drift(w))
                else:
                    corrupted_weights.append(w)
            else:
                corrupted_weights.append(w)
        return corrupted_weights

def main():
    fim = FIMCore(SEUProfile(bit_flip_rate=0.2)) # 20% rate for demonstration
    sample_weights = [0.5, -1.2, 3.14, 0.0, 100.0]

    print(f"Original weights: {sample_weights}")

    bit_flipped = fim.apply_faults(sample_weights, "bit_flip")
    print(f"Bit-flipped weights: {bit_flipped}")

    drifted = fim.apply_faults(sample_weights, "drift")
    print(f"Drifted weights: {drifted}")

if __name__ == "__main__":
    main()
