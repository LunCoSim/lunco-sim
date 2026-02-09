import numpy as np
from scipy.fft import fftn
from skimage.measure import shannon_entropy

class BioSignatureKernel:
    def __init__(self, spatial_res=(10, 10), spectral_bands=8, time_steps=16):
        self.shape = (*spatial_res, spectral_bands, time_steps)

    def _normalize(self, data):
        return (data - np.mean(data)) / (np.std(data) + 1e-8)

    def extract_features(self, raw_data):
        # Redimensiona para o hipercubo esperado
        try:
            data = self._normalize(raw_data.reshape(self.shape))
        except ValueError:
            # Fallback if raw_data size doesn't match
            expected_size = np.prod(self.shape)
            if raw_data.size < expected_size:
                padded = np.zeros(expected_size)
                padded[:raw_data.size] = raw_data
                data = self._normalize(padded.reshape(self.shape))
            else:
                data = self._normalize(raw_data[:expected_size].reshape(self.shape))

        # DNE (Dynamic Non-Equilibrium): Persistência temporal
        diff_t = np.diff(data, axis=-1)
        dne = np.tanh(np.mean(np.abs(fftn(diff_t))))

        # SSO (Spatial Self-Organization): Entropia espacial relativa
        sso_vals = [shannon_entropy(data[..., b, :]) for b in range(self.shape[2])]
        sso = np.mean(sso_vals) / (np.max(sso_vals) + 1e-8)

        # CDC (Cross-Domain Coupling): Correlação entre bandas
        bands = data.reshape(-1, self.shape[2], self.shape[3]).mean(axis=0)
        # Use simple correlation mean
        if self.shape[2] > 1:
            cdc = np.abs(np.corrcoef(bands)).mean()
        else:
            cdc = 0.5

        return {"D": float(dne), "S": float(sso), "C": float(cdc)}
