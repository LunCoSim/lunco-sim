import asyncio
import sys
import os
import numpy as np

# Add parent directory to path
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))

from avalon_harmonic_engine import AvalonNetwork, HarmonicInjector

async def test_propagation():
    print("Testing harmonic propagation...")
    network = AvalonNetwork()
    injector = HarmonicInjector("https://suno.com/s/test")
    await injector.decode_signal()

    initial_coherence = network.global_coherence
    print(f"Initial coherence: {initial_coherence}")

    await network.propagate_harmonic(injector.quantum_state, injector)

    final_coherence = network.global_coherence
    print(f"Final coherence: {final_coherence}")

    assert final_coherence > initial_coherence
    print("Test passed!")

if __name__ == "__main__":
    asyncio.run(test_propagation())
