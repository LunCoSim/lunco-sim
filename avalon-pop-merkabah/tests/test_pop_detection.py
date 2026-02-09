import sys
import os
import numpy as np

# Add parent directory to path
sys.path.append(os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))

from kernel import BioSignatureKernel
from quantum_logic import PersistentOrderOracle

def test_pop_logic():
    print("Testing POP logic...")
    kernel = BioSignatureKernel()
    oracle = PersistentOrderOracle()

    # Generate mock spectral data (10x10x8x16 = 12800 points)
    mock_data = np.random.rand(10, 10, 8, 16)

    features = kernel.extract_features(mock_data)
    print(f"Extracted features: {features}")

    psi_po = oracle.execute_and_filter(features)
    print(f"Psi_PO (Grover filtered): {psi_po}")

    assert 0 <= psi_po <= 1
    print("Test passed!")

if __name__ == "__main__":
    test_pop_logic()
