# Switch Tools — Linux

Porte não oficial para Linux do conjunto de ferramentas Switch (XCI/NSP/NSZ, etc.), mantido por luciano
para Linux, em Rust puro (egui, sem webview). App + ferramentas + dados vivem
numa pasta só, como no original.

## Funções

Converter NSP↔XCI, comprimir/decomprimir NSZ e XCZ, dividir (split), mesclar
(merge), aplicar update e patch, extrair firmware de XCI, extrair savegames,
gerar o NSP do forwarder NRO e formatar SD em FAT32 — as mesmas do Switch Tools.

## 1. Dependências (Arch)

Copie e cole:

```sh
sudo pacman -S --needed git rust make gcc python p7zip curl squashfs-tools
```

## 2. Compilar

Copie e cole (uma única linha):

```sh
git clone <url> switch-tools && cd switch-tools && ./setup.sh
```

No final a pasta `SWITCH_TOOLS/` sai pronta no repositório — AppImage + `bin/` +
`NSC_BUILDER/` juntos, autocontida e portátil. Artefatos intermediários são
apagados automaticamente; para recompilar, rode `./setup.sh` de novo.

## 3. Usar

Copie e cole (troque o caminho das chaves se preciso):

```sh
cp ~/prod.keys SWITCH_TOOLS/bin/                          # ou keys.dat — única coisa que você fornece
./SWITCH_TOOLS/Switch_Tools.AppImage
```

Os resultados caem ao lado do AppImage (`squirrel/`, `Firmware/` etc.). A pasta
`SWITCH_TOOLS/` pode ser movida para qualquer lugar (pendrive incluso).

Desenvolvimento (rodar sem compilar AppImage): `cargo run --manifest-path
src-tauri/Cargo.toml`. Montagem manual, passo a passo: `scripts/build-tools.sh`
→ `cargo build --release` (em `src-tauri/`) → `scripts/make-appimage.sh`.

Detalhes (fontes de cada ferramenta, quirk de chaves com 34 dígitos, override
de logo): ver [`bin/README.txt`](bin/README.txt).

## Créditos

Switch Tools original: **kempa** (psxtools.de). Ferramentas: SciresM (hactool,
hactoolnet), The-4n (4NXCI, hacBrewPack, hptnacp), julesontheroad
(NSC_BUILDER) e os mantenedores dos mirrors. Licenças em `bin/*_LICENSE`.
