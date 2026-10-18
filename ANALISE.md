# Switch Tools — análise (Linux port)

## O que é
Porte não oficial para Linux do **SAK v0.7.14** (kempa, psxtools.de) feito em **Rust + egui**, sem webview. Centraliza os mesmos fluxos do original: `XCI↔NSP`, `NSZ/XCZ`, `split/merge`, `patch`, `extract FW/save`, `NRO forwarder`, `FAT32` — via binários C nativos (`hactool`, `4nxci`, `renxpack`, `hacbrewpack`, `hptnacp`) e `squirrel` (Python/NSC_BUILDER).

- Pasta: `/home/luciano/Projetos/Switch-Tools`
- Versão do app: `0.7.14` (Cargo), `info_text` afirma `1.0`
- Plataforma: Linux x86_64

## Arquitetura (resumo)
- `src-tauri/src/lib.rs` (2076 linhas) — biblioteca: `app_dir()` AppImage-aware, `bin_dir()`, helpers `keys_status()`, `free_space_mb()`, fat32, split/merge, `build_plan()` por feature, `run_external()`, `tool_run()` e testes.
- `src-tauri/src/gui.rs` (1355 linhas) — egui: sidebar com 19 ferramentas + form + log.
- `src-tauri/src/main.rs` — entry.
- `scripts/build-tools.sh` — clona/monta `bin/` e `portable/` (NSC_BUILDER + venv).
- `scripts/make-appimage.sh` — empacota AppImage.
- `setup.sh` — pipeline automático.
- `SWITCH_TOOLS/` — pasta final portátil **gerada** (203 MB).

## Verificação de build
- `cargo check`: ✅ 25.6 s, sem erros.
- `cargo test`: ✅ **18 testes, 0 falhos** (part_naming, free space, hptnacp handling, bin_dir perms, command plan).
- `cargo clippy` / `cargo fmt`: não instalados para o toolchain.

## Bugs / inconsistências encontradas

| # | Severidade | Arquivo | Descrição |
|---|---|---|---|
| 1 | P1 (build) | `setup.sh` | Copia **`Switch Army Knife.AppImage`** (nome do make-appimage antigo) que não existe — o make-appimage gera **`Switch Tools.AppImage`**. Também move para `SAK_TOOLS/`, enquanto o README pede `SAK_TOOLS/`/`SAK_TOOLS`. Resultado: setup termina sem AppImage mesmo se o build for ok. |
| 2 | P1 (segurança) | `SWITCH_TOOLS/bin/` | Contém **`prod.keys` e `title.keys`** (chaves reais do console). Não listado em `.gitignore` (só `*.keys` ignora os nomes, mas a pasta inteira 203 MB precisava de ignore explícito do artefato de build). Se o repo virar git e for empurrado, vaze as chaves. Mitigação aplicada agora: adicionei `/SWITCH_TOOLS/` ao `.gitignore`. Revoke/re-gerar se já foi usado em outro lugar. |
| 3 | P2 (limpeza) | `bin/` | Mistura wrappers versionados (`README.txt`, `squirrel`) com artefatos não versionados de build (`prod.keys`, `title.keys`). O `.gitignore` ignora `bin/*` exceto README/squirrel/hactoolnet, OK, mas as chaves ficam no disco local — checar antes de `git add` broad. |
| 4 | P2 (nomes) | Projeto | Título varia: `Switch Tools`, `SAK`, `Switch Army Knife`, `sak-linux` (Cargo bin), `Switch Tools.AppImage`, `SAK_TOOLS`. Unificar em README/setup/AppImage. |
| 5 | P2 (versão) | `info_text` | Diz `Version: 1.0`, Cargo `0.7.14`; README diz "Linux port". Sincronizar. |
| 6 | P3 (UX) | `lib.rs:extract-save` | A extração de save usa `hactool -t save` e diretório `tmp`; se a chave for inválida aborta silenciosamente? Verificar mensagens de erro no log do squirrel (via `run_external`). |
| 7 | P3 (compat) | build-tools.sh | SAK 7z de mirror `dezem/SAK` pode ficar offline; `hactoolnet.exe` (NativeAOT) não roda nativamente no Linux (usa wrapper Wine? Comentário indica wrapper, não implementado no launcher). A extração de save já usa `hactool -t save` nativo, então ok. |
| 8 | P3 (redes) | build | Pip instala NSC_BUILDER em venv; aviso de pip removido e nutdb.json removido (baixa on demand). Para builds offline: deixa de funcionar. |
| 9 | P3 (docs) | SAK original | README aponta `~prod.keys` como único input do usuário — documentar também `title.keys` se o forwarder de NRO precisar. |

## Pontos fortes
- Build reproduzível (cargo, scripts detallhados).
- `app_dir()` lê `APPIMAGE` e usa o `bin/` relativo → real portável.
- Testes de planos de comando guardam compatibilidade com script original line-by-line.
- Secret ignore de `*.keys` presente — manter.

## Recomendações imediatas
1. Corrigir `setup.sh` (nomenclatura do AppImage e pasta final).
2. Renomear consistentemente para `SWITCH_TOOLS` → `Switch-Tools` e regravar bin/título.
3. Remover/mover `SWITCH_TOOLS/` para fora do repo (`~/.local/share/Switch-Tools` / pendrive) + adicionar ao .gitignore ✅ (feito hoje).
4. Rotacionar as chaves em `Switch-Tools/SWITCH_TOOLS/bin/*.keys` se foram copiadas de outro lugar ou versionadas.
5. Adicionar README curto "dev quickstart" com `cargo run --manifest-path src-tauri/Cargo.toml`.
6. Smoke test de cada feature em hardware real (minha sugestão: extrair FW, update XCI com 1 update) usando um XCI de testes antes de publicar.

## Correções aplicadas (esta sessão)
- `setup.sh`: alinhado para copiar `Switch_Tools.AppImage` (antigo `Switch Army Knife.AppImage`).
- `scripts/make-appimage.sh`: saída renomeada para `Switch_Tools.AppImage` (sem espaço).
- `README.md`: clone para `switch-tools`, execução passa a usar `SWITCH_TOOLS/Switch_Tools.AppImage`.
- `src-tauri/src/lib.rs`: `Version: 0.7.14` (era `1.0`) alinhado ao Cargo.
- `.gitignore`: adicionado `/SWITCH_TOOLS/` para não versionar build portátil com chaves do usuário.
- Build/check: `cargo check` and `cargo test` (18) verdes.
