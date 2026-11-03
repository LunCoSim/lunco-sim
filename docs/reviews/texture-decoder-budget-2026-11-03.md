# Bounded texture decoder policy

The NASA unsigned LOLA 64 ppd source is 530,934,146 bytes. The image library's
previous default allocation budget rejected its output plus intermediate TIFF
decoder storage with `Memory limit exceeded`. The native reader now receives
an explicit bounded limit before decoding. A 1 GiB run successfully prepared
16,384 × 8,192 grayscale 16-bit PNG data without a gamma conversion.

Policy precedence: a texture recipe's positive integer `max_decode_bytes`,
then its owning Twin's `assets.texture_decode_max_bytes`, then persistent
`asset_processing.texture_decode_max_bytes` (default 1 GiB). Settings > Data &
libraries > Texture preparation edits the application value, including 1 GiB
and 2 GiB presets. Twin settings uses the existing generic native editor.
The worker snapshots the owning Twin by dataset root at admission. Focus on
another Twin does not redirect the job's policy. CLI processing uses the same
resolver. New settings affect the next job; Rebuild explicitly reruns an
already valid artifact. This controls decoding, not every application or
resampling allocation, and never retries with unlimited memory.

Focused tests passed for global/Twin precedence and invalid values, the actual
reader's low-budget failure, per-asset override, and 16-bit sample preservation.
The DEBUG application build and standalone asset CLI check passed.
Native checks passed in headful session 4493, PID 2607081, executable
6def8e5f79d8039d6f5c803c9a7a710cc832dae6e738edf64e50f11adceae901.
The Settings editor displayed 1024 MiB, then 2048 MiB after clicking its 2 GiB
preset, and was restored to 1 GiB. An actual 1-byte Twin override failed the
LOLA decoder with `Memory limit exceeded`; restoring the Twin value to 1 GiB
successfully rebuilt the same source. No per-asset override was present.
The first test driver's 30-second deadline expired during a valid preparation;
the corrected 180-second gate passed both cases without changing the decoder.
Native evidence is retained with Griffin V13 R4: texture-settings-native-r4.png,
texture-settings-2gib-native-r4.png, texture-budget-native-gate-r4.json.
The output is 186,234,723 bytes, SHA256
cfcd5f181016c8d8272079a0ee3bd165620f28a882b074e4641bcee3194375dc.

The user approved work locally without Trello; nothing has been pushed.
