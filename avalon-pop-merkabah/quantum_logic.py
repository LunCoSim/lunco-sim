from qiskit import QuantumCircuit, QuantumRegister, ClassicalRegister
from qiskit_aer import Aer
from qiskit.circuit.library import GroverOperator

class PersistentOrderOracle:
    def __init__(self, threshold=0.75):
        self.threshold = threshold
        self.target_state = "111"

    def _build_oracle(self, qr):
        """
        Inverte a fase do estado de alvo (Sinal de Vida).
        """
        oracle_qc = QuantumCircuit(qr)
        # No nosso design fiction, 111 é a ressonância harmônica perfeita
        oracle_qc.cz(qr[0], qr[2])
        oracle_qc.x(qr[1])
        oracle_qc.cz(qr[0], qr[1])
        oracle_qc.x(qr[1])
        return oracle_qc

    def build_filtered_circuit(self, features):
        qr = QuantumRegister(3, 'q_features')
        cr = ClassicalRegister(3, 'measure')
        qc = QuantumCircuit(qr, cr)

        # 1. Preparação: Codificação baseada nas features do Kernel
        for i, key in enumerate(['D', 'S', 'C']):
            angle = (features[key] / self.threshold) * (3.14 / 2)
            qc.ry(min(angle, 3.14), qr[i])

        # 2. Noise Filtering via Grover (1 iteração para 3 qubits é ideal)
        oracle = self._build_oracle(qr)
        grover_op = GroverOperator(oracle)

        qc = qc.compose(grover_op)

        # 3. Medição da Probabilidade de Coerência
        qc.measure(qr, cr)
        return qc

    def execute_and_filter(self, features):
        qc = self.build_filtered_circuit(features)
        backend = Aer.get_backend('qasm_simulator')

        from qiskit import transpile
        tqc = transpile(qc, backend)

        job = backend.run(tqc, shots=1024)
        result = job.result().get_counts()

        # O "Sinal de Vida" é a contagem do estado '111'
        psi_po = result.get(self.target_state, 0) / 1024
        return psi_po
