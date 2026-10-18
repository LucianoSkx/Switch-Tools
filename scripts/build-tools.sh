#!/usr/bin/env bash
# Monta bin/: compila os helpers C/C++ das fontes, baixa os dados do Switch Tools Windows
# (hactoolnet.exe NativeAOT + fw_nca_mapping.csv + assets do forwarder) e prepara
# o squirrel (venv portátil). Fontes clonadas em .build/ (dentro do repositório).
set -u

REPO="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO/bin"
NSB="$REPO/NSC_BUILDER"
SRC="$REPO/.build"
SAK7Z_URL="https://github.com/dezem/SAK/releases/download/0.7.14/SAK_v0.7.14_64bit_20220405-19-38-49.7z"
SAK7Z="/tmp/SAK_v0.7.14_64bit.7z"
mkdir -p "$BIN" "$NSB/py" "$SRC"
status=0
dir=""

clone() { # <name> <url> -> seta $dir
  dir="$SRC/$1"
  if [ ! -d "$dir/.git" ]; then
    git clone --depth 1 "$2" "$dir" >/dev/null 2>&1 || { echo "FAIL  $1: clone of $2 failed"; return 1; }
  fi
}

build_make() { # <name> <url> <binário, relativo ao repo> <destino em bin/>
  local name="$1" url="$2" bin="$3" dest="$4" wd
  clone "$name" "$url" || { status=1; return; }
  # hactool: fixa na versão da era do Switch Tools (1jun2020); master é mais estrito
  # com nomes de chaves e aborta em dumps que o Switch Tools original tolerava.
  if [ "$name" = hactool ]; then
    git -C "$dir" fetch -q --unshallow 2>/dev/null
    git -C "$dir" checkout -q 3121a5b
  fi
  # hactool/renxpack: Makefile inclui config.mk (só o template vem no repo)
  if [ ! -f "$dir/config.mk" ] && [ -f "$dir/config.mk.template" ]; then
    cp "$dir/config.mk.template" "$dir/config.mk"
  fi
  wd="$dir"
  case "$bin" in
    */*) wd="$dir/$(dirname "$bin")"; bin="$(basename "$bin")" ;;
  esac
  if ! make -C "$wd" -j"$(nproc)" >"/tmp/sak-build-$name.log" 2>&1; then
    echo "FAIL  $name: make (log: /tmp/sak-build-$name.log)"
    status=1
    return
  fi
  if [ ! -f "$wd/$bin" ]; then
    echo "FAIL  $name: binary $bin was not produced (log: /tmp/sak-build-$name.log)"
    status=1
    return
  fi
  install -m 755 "$wd/$bin" "$BIN/$dest"
  strip "$BIN/$dest" 2>/dev/null
  echo "OK    $name -> bin/$dest"
}

# 1. Helpers C/C++ (The-4n apagou a conta; mirrors preservam os originais)
build_make hactool     https://github.com/SciresM/hactool        hactool                                  hactool
build_make 4nxci       https://github.com/tiliarou/4NXCI         4nxci                                    4nxci
build_make renxpack    https://github.com/tiliarou/reNXpack      renxpack                                 renxpack
build_make hacbrewpack https://github.com/rlaphoenix/hacBrewPack hacbrewpack                              hacbrewpack
build_make hptnacp     https://github.com/46620/hacpack          hacPack-Tools/hacPackTools-NACP/hptnacp  hptnacp

# 2. Dados do SAK Windows: hactoolnet (NativeAOT, fonte apagada → wrapper Wine),
#    fw_nca_mapping.csv, assets do NRO forwarder e licenças
if [ ! -f "$SAK7Z" ]; then
  curl -sL -o "$SAK7Z" "$SAK7Z_URL" || echo "FAIL  sak-data: SAK 7z download failed"
fi
if [ -f "$SAK7Z" ]; then
  ok=1
  for f in fw_nca_mapping.csv nro_fw_NintendoLogo.png nro_fw_StartupMovie.gif \
           nro_fw_main nro_fw_main.npdm 4nxci_LICENSE hacbrewpack_LICENSE renxpack_LICENSE; do
    7z e -y -o"$BIN" "$SAK7Z" "SAK_64bit/bin/$f" >/dev/null || ok=0
  done
  if [ "$ok" = 1 ]; then
    echo "OK    sak-data -> bin/ (fw_nca_mapping.csv + nro_fw_* + licenses)"
  else
    echo "FAIL  sak-data: extraction (see log above)"
    status=1
  fi
fi

# 3. Squirrel: NSC_BUILDER + venv portátil com nsz
nsb="$NSB"
# Se o source já veio do repositório (versão patchada), não re-clona do upstream.
if [ ! -f "$nsb/py/ztools/squirrel.py" ]; then
  git clone --depth 1 https://github.com/julesontheroad/NSC_BUILDER "$nsb" >/dev/null 2>&1 || { echo "FAIL  squirrel: clone"; status=1; }
fi
# py3.11+: str(IntEnum) devolve o valor; o NSC_BUILDER (py3.8) compara
# str(contentType) == 'Content.META' em 372 pontos → restaura o __str__ antigo.
python3 - "$nsb/py/ztools/Fs/Type.py" <<'PYEOF' || status=1
import sys
p = sys.argv[1]
s = open(p).read()
if 'def __str__' not in s:
    s = s.replace("\tPUBLIC_DATA = 0x5\n",
                  "\tPUBLIC_DATA = 0x5\n\n\t# py3.11+ faz str(IntEnum) devolver o valor; NSC_BUILDER compara 'Content.META'\n\tdef __str__(self):\n\t\treturn 'Content.' + self.name\n", 1)
    open(p, 'w').write(s)
PYEOF
venv="$NSB/py/venv"
if [ ! -x "$venv/bin/python" ]; then
  python3 -m venv "$venv" || { echo "FAIL  squirrel: venv"; status=1; }
fi
# NSC_BUILDER usa cls nos progressos; converter para terminal Unix.
sed -i "s/os.system('cls')#windows/os.system('clear')  # linux/g" "$nsb/py/ztools/lib/secondary.py" 2>/dev/null || true
# nutdb.py baixa 87 MB do DB de renomeação só de ser importado + makedirs(DB);
# Switch Tools não usa DB: corta o download e as pastas criadas em toda execução.
python3 - "$nsb/py/ztools" <<'PYEOF' || status=1
import sys, re
p = sys.argv[1]
for f in ("squirrel.py", "lib/nutdb.py", "lib/DBmodule.py"):
    path = f"{p}/{f}"
    s = open(path).read()
    s = re.sub(r'\t*try:\n\t*check_current\(\)\n\t*except:pass\s*\n', '', s)
    s = re.sub(r'\t*if not os\.path\.exists\(DATABASE_folder\):\n\t*os\.makedirs\(DATABASE_folder\)\s*\n', '', s)
    open(path, 'w').write(s)
PYEOF
if [ -x "$venv/bin/pip" ]; then
  if "$venv/bin/pip" -q install urllib3 unidecode tqdm bs4 pycryptodome pykakasi googletrans \
      chardet zstandard colorama requests pillow nsz >/dev/null 2>&1; then
    echo "OK    squirrel -> bin/squirrel + NSC_BUILDER/ (ztools + venv)"
  else
    echo "FAIL  squirrel: pip (network?)"
    status=1
  fi
fi

# 4. Peso: remove .git, docs não usados e __pycache__; pip sai do venv entregue
#    (recreie o venv com `rm -rf NSC_BUILDER/py/venv` se mudar as dependências).
rm -rf "$nsb/.git" "$nsb/py/Documentation and Resources" "$nsb/py/zconfig" \
       "$nsb/py/ztools/web" "$nsb/py/ztools/_bottle_websocket_" "$nsb/py/ztools/manager"
# remove Windows-only entry points/installs (app drives squirrel.py from bin/squirrel)
find "$nsb" -type f \( -iname "*.bat" -o -iname "*.cmd" -o -iname "*.exe" \) \( -path "*ztools*" -o -path "*zconfig*" \) -delete 2>/dev/null
# remove MTP/Windows transfer modules not used by Switch Tools
rm -rf "$nsb/py/ztools/mtp" 2>/dev/null
find "$NSB" -name __pycache__ -type d -prune -exec rm -rf {} + 2>/dev/null
[ -x "$venv/bin/pip" ] && "$venv/bin/pip" -q uninstall -y pip >/dev/null 2>&1
echo "OK    slim -> NSC_BUILDER/ without .git, docs, DB auto-download (nutdb.py patch), __pycache__, pip"

echo
echo "Wrappers in the repository: bin/squirrel (portable venv) and bin/hactool (native)."
exit $status
