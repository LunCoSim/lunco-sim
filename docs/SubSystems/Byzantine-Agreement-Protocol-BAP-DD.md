# Byzantine Agreement Protocol with Drift Detection (BAP-DD)

## 1. Mathematical Foundation
BAP-DD is an "immune response" for the Mesh-Neuron architecture, tolerating $f=1$ Byzantine fault in an $N=4$ granule system.

### 1.1 Threat Model (FIM Mapping)
- **Class A (Confident Hallucinator):** Detected via impossible certainty ($H(m_i) \approx 0$) and model parameter divergence.
- **Class B (Entropy Storm):** Detected via high temporal variance in confidence ($\sigma(H(m_i)) > \tau$).
- **Class C (Silent Drift):** Detected via systematic bias over time ($D_{ij} > \delta_{\text{critical}}$).

## 2. Hybrid Logical Clocks (HLC)
To handle relativistic effects and clock skew, BAP-DD uses a tuple `(pt, l, c)`:
- `pt`: Physical time (Atomic clock).
- `l`: Logical time (Monotonically increasing).
- `c`: Causal identifier (Node ID).
Causal consistency is maintained by using logical time `l` for drift calculations and temporal authority.

## 3. Drift Detection: Exponentially Weighted Moving Consensus (EWMC)
Each granule maintains a consensus drift vector for every other granule:
$$D_{ij}(t) = \alpha \cdot D_{ij}(t-1) + (1-\alpha) \cdot \|m_i(t) - m_j(t)\|_2$$
- **Alert Threshold:** $D_{ij}(t) > 0.15$ over 6 hours flags a granule as "drifting".
- **Authority Weighting:** $w_j(t) = 1 / (1 + \beta \cdot \sum D_{ij}(t))$. Weight decays exponentially as drift accumulates.

## 4. Partition Priest Protocol (Split-Brain Resolution)
In the event of a network partition (e.g., solar storm):
1. **Authority Gradient:** Granules have a static priority order (α > β > γ > δ).
2. **Partition Claims:** Sub-groups operate autonomously but broadcast "Partition Claims" signed with their highest-priority key.
3. **Reconnection:** The sub-group with the highest-priority granule (e.g., Module Alpha) has its timeline adopted as canonical.
4. **Archiving:** The losing sub-group's timeline is archived as a "historical alternate" in KARNAK.

## 5. Consensus State Machine
- **Normal Operation:** Standard weighted median consensus.
- **Judgment Hysteresis:** Triggered by high variance ($\sigma(H(m_i)) > \tau$); buffers proposals for 40 min (Mars round-trip).
- **Hard Freeze:** Triggered by Φ ≥ 0.80 or critical Byzantine behavior.
- **Stochastic Resolution:** Entropy-weighted randomized selection as a last resort in life-threatening emergencies.

## 6. Antarctic Validation: "Split-Fleet Scenario"
The protocol is validated by physically partitioning the Antarctic station network and presenting sub-groups with identical emergencies. The test verifies:
- Local consensus with Class C fault marginalization.
- Deterministic timeline selection via the Partition Priest Protocol upon reconnection.
- Causal history integrity via HLC.
