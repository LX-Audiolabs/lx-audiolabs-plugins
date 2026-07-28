# audio-graph agent summary

project: **lx-audiolabs-slint**  
generated: `2026-07-28T11:56:08Z`  
version: 0.3.2  
nodes: 13 · edges: 36 · findings: 2

## frameworks in use
baseview, clap, lx-slint-editor, raw-window-handle, slint, truce, truce-slint

## rules
- **crate_vs_plugin**: Reusable logic belongs in crates/, product logic belongs in plugins/.
- **plugin_target_editor**: Plugins should use lx-slint-editor, not truce-slint.

## migration
plugins: 6 · legacy: 0 · migrated: 6

- **truce-slint** `truce-slint`→`lx-slint-editor`: all 6 migrated

## plugins
- **aether** `plugins/aether` [migrated] fw=`clap+lx-slint-editor+slint+truce` deps=[lx-analysis, lx-dsp, lx-slint-build, lx-slint-editor] logic=Aether params=AetherParams(26) ipc=shared_state slint_comp=16 roles=audio,build,entry,manifest,slint,state,ui
- **aurum-slint** `plugins/aurum-slint` [migrated] fw=`clap+lx-slint-editor+slint+truce` deps=[lx-analysis, lx-dsp, lx-slint-build, lx-slint-editor] logic=Aurum params=AurumParams(70) ipc=shared_state slint_comp=22 roles=audio,build,entry,manifest,slint,ui
- **equilibrium** `plugins/equilibrium` [migrated] fw=`clap+lx-slint-editor+slint+truce` deps=[lx-analysis, lx-dsp, lx-slint-build, lx-slint-editor] logic=Equilibrium params=EquilibriumParams(29) ipc=shared_state slint_comp=20 roles=audio,build,entry,manifest,slint,state,ui
- **lucent** `plugins/lucent` [migrated] fw=`clap+lx-slint-editor+slint+truce` deps=[lx-analysis, lx-dsp, lx-slint-build, lx-slint-editor] logic=Lucent params=LucentParams(5) ipc=relay,shared_state,shm slint_comp=17 roles=audio,build,entry,ipc,manifest,slint,ui
- **lucent-relay** `plugins/lucent-relay` [migrated] fw=`clap+lx-slint-editor+slint+truce` deps=[lx-analysis, lx-dsp, lx-slint-build, lx-slint-editor] logic=LucentRelay params=LucentRelayParams(2) ipc=relay,shared_state,shm slint_comp=4 roles=audio,build,entry,manifest,slint,ui
- **meridian** `plugins/meridian` [migrated] fw=`clap+lx-slint-editor+slint+truce` deps=[lx-analysis, lx-dsp, lx-slint-build, lx-slint-editor] logic=Meridian params=MeridianParams(40) ipc=shared_state slint_comp=25 roles=audio,build,entry,manifest,slint,state,ui

## crates
- **lx-analysis** `crates/lx-analysis` deps=[lx-shm, lx-vault] api=12 ipc=relay,shared_state,shm
- **lx-dsp** `crates/lx-dsp` deps=[] api=19 process_methods=11
- **lx-shm** `crates/lx-shm` deps=[] api=6 ipc=relay,seqlock,shm
- **lx-slint-build** `crates/lx-slint-build` deps=[] api=2
- **lx-slint-editor** `crates/lx-slint-editor` deps=[] api=2
- **lx-ui-slint** `crates/lx-ui-slint` deps=[lx-slint-build] slint_export=38
- **lx-vault** `crates/lx-vault` deps=[] api=9

## edges
### depends_on
- `lx-analysis` → `lx-shm`
- `lx-analysis` → `lx-vault`
- `aether` → `lx-analysis`
- `aether` → `lx-dsp`
- `aether` → `lx-slint-editor`
- `aurum-slint` → `lx-analysis`
- `aurum-slint` → `lx-dsp`
- `aurum-slint` → `lx-slint-editor`
- `equilibrium` → `lx-analysis`
- `equilibrium` → `lx-dsp`
- `equilibrium` → `lx-slint-editor`
- `lucent` → `lx-analysis`
- `lucent` → `lx-dsp`
- `lucent` → `lx-slint-editor`
- `lucent-relay` → `lx-analysis`
- `lucent-relay` → `lx-dsp`
- `lucent-relay` → `lx-slint-editor`
- `meridian` → `lx-analysis`
- `meridian` → `lx-dsp`
- `meridian` → `lx-slint-editor`
### build_depends_on
- `lx-ui-slint` → `lx-slint-build`
- `aether` → `lx-slint-build`
- `aurum-slint` → `lx-slint-build`
- `equilibrium` → `lx-slint-build`
- `lucent` → `lx-slint-build`
- `lucent-relay` → `lx-slint-build`
- `meridian` → `lx-slint-build`
### uses_ui
- `aether` → `lx-ui-slint` — 14 shared Lx* components
- `aurum-slint` → `lx-ui-slint` — 20 shared Lx* components
- `equilibrium` → `lx-ui-slint` — 17 shared Lx* components
- `lucent` → `lx-ui-slint` — 15 shared Lx* components
- `lucent-relay` → `lx-ui-slint` — 3 shared Lx* components
- `meridian` → `lx-ui-slint` — 23 shared Lx* components
### ipc_peer
- `lucent` → `lucent-relay` — shared: relay, shm
### runtime_depends_on
- `lucent` → `lx-shm` — via relay+shm
- `lucent-relay` → `lx-shm` — via relay+shm

## findings (error=0 warn=0 info=2)
- [INF] **dsp_process_methods** `lx-dsp`: lx-dsp has 11 methods named process (DSP units, not plugin hooks)
- [INF] **large_param_surface** `aurum-slint`: aurum-slint exposes 70 visible params (70 total) — state migration / UI binding cost is high

## delta
_no structural changes since previous graph._

## agent usage
1. Read this file first (token-cheap).  
2. Check **delta** / `audio-graph.delta.md` for what changed since last gen.  
3. Open `audio-graph.json` only for params_fields, public_api, or full edges.  
4. Use graphify only for deep non-audio symbol searches.  
5. Scope: `rust-audio-graph --plugin <name> .` for one-hop slice.  

