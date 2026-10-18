Switch Tools (Linux) — pasta bin/
======================================

O app procura SOMENTE nesta pasta, ao lado dele mesmo (mesma lógica do Switch Tools
com $CUR_PATH\bin) — tudo é portátil. `scripts/build-tools.sh` monta tudo
aqui (clona fontes em .build/, compila, baixa dados do Switch Tools Windows).

Nomes exatos esperados pelo app:

  hactool      extrai FW de XCI         https://github.com/SciresM/hactool        C, make
  4nxci        converte XCI para NSP    https://github.com/tiliarou/4NXCI         C, make
  renxpack     patch de NSP             https://github.com/tiliarou/reNXpack      C, make
  hacbrewpack  gera o NSP do forwarder  https://github.com/rlaphoenix/hacBrewPack C, make
  hptnacp      cria control.nacp        https://github.com/46620/hacpack          C++, make
               (subpasta hacPack-Tools/hacPackTools-NACP)
  (extração de save usa hactool com -t save; não é preciso hactoolnet/Wine)
  squirrel     update/compress/convert  wrapper para portable/venv + NSC_BUILDER
               (Python 3 + nsz, tudo em portable/ ao lado de bin/).
               Na primeira execução cria symlink keys.txt -> prod.keys/keys.dat.

O The-4n apagou a conta; os cinco builders C/C++ usam mirrors fiéis.

Dados já inclusos (extraídos do Switch Tools v0.7.14 oficial pelo build script):
  fw_nca_mapping.csv, nro_fw_main, nro_fw_main.npdm, nro_fw_NintendoLogo.png,
  nro_fw_StartupMovie.gif, licenças das ferramentas.

Único arquivo que falta (usuário): suas chaves
  prod.keys ou keys.dat   em bin/ (quase tudo precisa)
  Nota: packs recentes (ex.: ProdKeys.net) trazem 33 chaves master_kek/mariko
  com um "00" extra no fim (34 dígitos); o hactool recusa o arquivo por isso
  (erro "must be 32 hex digits"). Se aparecer, remova o "00" final dessas
  linhas — as demais chaves ficam intactas.

Override de logo (opcional): pasta custom/ ao lado do app, com NintendoLogo.png
e/ou StartupMovie.gif — substitui os nro_fw_* correspondentes.

Saídas (squirrel/, Firmware/, extracted_save_*, nroforwarder/) ficam ao lado do
app — nunca dentro do AppImage montado em /tmp.
