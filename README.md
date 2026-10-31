<div align="center">

# ◆ odotui

**Tu entorno de desarrollo Odoo en la terminal.**<br>
Bases de datos, backups, servidor y módulos: con el mouse, con el teclado o con una IA.

[![Rust](https://img.shields.io/badge/Rust-2024-f74c00?logo=rust&logoColor=white)](https://www.rust-lang.org)
[![ratatui](https://img.shields.io/badge/ratatui-0.30-bb9af7)](https://ratatui.rs)
[![MCP](https://img.shields.io/badge/MCP-servidor-8fd6a4)](https://modelcontextprotocol.io)
[![Odoo](https://img.shields.io/badge/Odoo-docker%20%7C%20local-714B67?logo=odoo&logoColor=white)](https://www.odoo.com)
[![Linux](https://img.shields.io/badge/Linux-✓-e8b86d?logo=linux&logoColor=black)](#-instalación)

<img src="docs/screenshots/hero.png" alt="odotui: bases de datos con su versión de Odoo y el detalle de la seleccionada" width="900">

</div>

## ✨ Qué hace

| | |
|---|---|
| 🗄️ **Bases de datos** | Lista con tamaño, dueño y versión de Odoo. Duplica, renombra, neutraliza y borra, siempre junto con su filestore. |
| ♻️ **Restaurar backups** | Carga `dump.sql` + `filestore/` con **progreso real**, se puede cancelar y neutraliza la copia (sin crons, sin correo, `admin/admin`). |
| 🖥️ **Instancia** | Inicia, para y reinicia Odoo (contenedor o `odoo-bin`) con el **log en vivo**: colores por nivel, tracebacks resaltados y filtro. |
| 🧩 **Módulos** | Marca módulos y los instala (`-i`) o actualiza (`-u`) viendo la salida de Odoo. |
| 🤖 **IA por MCP** | Claude Code, opencode o Codex manejan odotui entero: restauran, actualizan, corren tests y leen tracebacks. **Si la TUI está abierta, lo ves en vivo.** |
| 🖱️ **Mouse o teclado** | Todo se puede hacer con clics (pestañas, filas, casillas, botones, rueda) o con teclas. Cada botón muestra su tecla. |
| 🐳 **Docker o local** | Perfiles para `docker compose` o para Odoo desde el código fuente con su Postgres. |
| 🎨 **Sigue tu tema** | En [Omarchy](https://omarchy.org) usa la paleta del sistema y cambia con ella; incluye tokyonight, catppuccin y gruvbox. |

## 📸 Capturas

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/restore.png" alt="Restauración en curso con barra de progreso"></td>
    <td width="50%"><img src="docs/screenshots/instance.png" alt="Log de Odoo en vivo con un traceback resaltado"></td>
  </tr>
  <tr>
    <td align="center"><b>♻️ Restaurar</b>: progreso real del <code>dump.sql</code>, pasos y log</td>
    <td align="center"><b>🖥️ Instancia</b>: log en vivo con los errores y tracebacks en rojo</td>
  </tr>
</table>

<div align="center">
  <img src="docs/screenshots/ai.png" alt="Una IA corriendo los tests de un módulo a través de odotui" width="900">
  <p><b>🤖 Una IA trabajando a través de odotui</b>: corrió los tests de <code>acme_ventas</code>; la TUI saltó a Módulos,<br>muestra <code>IA</code> en la cabecera y el fallo con su traceback, el mismo que recibe la IA.</p>
</div>

## 🚀 Instalación

```bash
git clone https://github.com/jorge07RD/odotui.git
cd odotui
cargo install --path . --locked
```

Necesitas `psql` en el `PATH`. Para perfiles docker, además, `docker` sin `sudo` (tu usuario en el grupo `docker`).

> [!TIP]
> Los íconos usan una [Nerd Font](https://www.nerdfonts.com). Si tu terminal no tiene una, pon `icons = false` en la config.

## ⚡ Primeros pasos

```bash
odotui            # sin config, abre el formulario para crear el primer perfil
odotui --init     # o escribe un ejemplo en ~/.config/odotui/config.toml
```

1. Crea un perfil en **5 Perfiles** y pulsa **Probar** (`t`).
2. En **2 Restaurar** elige un backup y pulsa `Enter`.
3. Lánzalo en **3 Instancia** (`s`) y ábrelo en el navegador (`o`).

## 🤖 IA (MCP)

`odotui mcp` es un servidor [MCP](https://modelcontextprotocol.io) por stdio. Regístralo una vez:

```bash
claude mcp add -s user odotui -- odotui mcp
```

En otros clientes, la entrada equivalente es:

```json
{ "mcpServers": { "odotui": { "command": "odotui", "args": ["mcp"] } } }
```

```mermaid
flowchart LR
    IA["🤖 Claude Code / opencode / Codex"] -->|MCP stdio| M["odotui mcp"]
    M -->|¿TUI abierta?| S[("socket local")]
    S -->|sí| T["🖥️ TUI: pestaña, progreso y log en vivo"]
    M -->|no| H["modo sin interfaz"]
    T --> N["mismo núcleo"]
    H --> N
    N --> PG[("Postgres")]
    N --> O["Odoo"]
```

| | Herramienta | Para qué |
|---|---|---|
| 🧭 | `status` | Perfil activo, rutas, estado de Odoo y si la TUI está abierta |
| 🔀 | `switch_profile` · `show` · `cancel` | Cambiar de perfil, mostrar una pestaña, cancelar la tarea en curso |
| 📋 | `list_databases` · `list_backups` · `list_modules` | Inventario |
| 🔍 | `sql_query` | SQL con psql, **de solo lectura** salvo `write=true` |
| 📜 | `read_logs` | Log de Odoo; con `errors_only` devuelve cada error con su traceback |
| ♻️ | `restore_backup` · `duplicate_database` · `rename_database` · `neutralize_database` · `drop_database` | Lo mismo que las pestañas BDs y Restaurar |
| 🧩 | `install_modules` · `update_modules` | `-i` / `-u` con `--stop-after-init`; devuelven los errores del log |
| 🧪 | `run_tests` | `--test-enable --test-tags`; devuelve el resumen y cada fallo con su traceback |
| 🐍 | `odoo_shell` | Python con `env` del ORM (rollback salvo `env.cr.commit()`) |
| 🏗️ | `scaffold_module` | `odoo scaffold` en `addons_dir` |
| ⏯️ | `instance` | start · stop · restart |

El ciclo típico de la IA es: editar el módulo → `update_modules` → `run_tests` → `read_logs` con `errors_only`.

> [!NOTE]
> Solo corre una tarea larga a la vez, tuya o de la IA; la otra recibe «ocupado». La TUI y `odotui mcp` se hablan por `$XDG_RUNTIME_DIR/odotui.sock`, con permisos `0600`.

> [!WARNING]
> `drop_database`, `restore_backup` con `overwrite` y `sql_query` con `write=true` son irreversibles. Las instrucciones del servidor piden a la IA confirmarlo contigo antes.

## ⚙️ Perfiles

`~/.config/odotui/config.toml` (ver [`config.example.toml`](config.example.toml)):

```toml
default_profile = "docker-17"

[[profile]]
name = "docker-17"
mode = "docker"
pg_host = "db"                    # host de Postgres visto desde Odoo
pg_user = "odoo"
pg_password = "odoo"
pg_container = "odoo17-db-1"      # psql corre aquí dentro
odoo_container = "odoo17-web-1"
backups_dir = "/home/yo/backups"  # <carpeta>/dump.sql (+ filestore/), en este equipo
addons_dir = "/mnt/extra-addons"  # para scaffold, ruta en el contenedor

[[profile]]
name = "local-17"
mode = "local"
pg_host = "localhost"
pg_user = "odoo"
odoo_bin = "/home/yo/odoo17/odoo-bin"
python = "/home/yo/odoo17/.venv/bin/python"
odoo_conf = "/home/yo/odoo17/odoo.conf"
backups_dir = "/home/yo/backups"
```

- 🐳 **docker**: `psql` corre dentro de `pg_container`. Odoo se maneja con `docker start/stop/restart`; los módulos, con `docker exec <odoo_container> odoo …`. El filestore se copia con `docker cp` y se le hace `chown odoo:odoo`.
- 💻 **local**: `odoo-bin` corre **en segundo plano**, con su pid y su log en `~/.local/state/odotui/`. Sobrevive a la TUI y la IA ve la misma instancia. Pararlo le manda SIGINT para que cierre limpio.

## ♻️ Restauración paso a paso

1. Valida el nombre. Si la BD existe, pide confirmación, cierra sus conexiones y hace `DROP`.
2. `CREATE DATABASE … ENCODING 'UTF8' TEMPLATE template0`.
3. `psql -q` con el dump por stdin, con progreso por bytes. Igual que Odoo, no se detiene en errores inocuos: los cuenta y los marca en rojo.
4. Copia `filestore/` a `<filestore_dir>/<bd>`.
5. Neutraliza: desactiva crons, servidores de correo y pagos, quita el código enterprise, regenera `database.uuid`, apunta `web.base.url` a `odoo_url`, marca `database.is_neutralized` y pone la contraseña de `admin` (hash `pbkdf2-sha512` compatible con passlib).

`X` o **Cancelar** matan `psql` y avisan de que la BD quedó a medias.

## ⌨️ Mouse y teclado

| 🖱️ Mouse | ⌨️ Teclado | Acción |
|---|---|---|
| clic en una pestaña | `Tab` · `1`-`5` | cambiar de herramienta |
| clic en el perfil ▾ | `p` | siguiente perfil |
| clic en una fila; 2º clic | `↑↓` `jk` · `Enter` | seleccionar · abrir |
| clic en `[ ]` | `Espacio` | marcar un módulo |
| clic en un botón | la tecla de su tapa | su acción |
| rueda o arrastre | `PgUp` `PgDn` · `Home` `End` | scroll |
| clic en `⇣ siguiendo` | `f` | seguir el final del log |
| clic en la barra de filtro | `/` · `Esc` | filtrar · borrar el filtro |
| `≡` | `:` | paleta de comandos (sin importar tildes) |
| `? ayuda` · `q salir` | `?` · `q` | ayuda · salir |

## 📁 Archivos

| Ruta | Qué guarda |
|---|---|
| `~/.config/odotui/config.toml` | perfiles, tema (`theme`) e íconos (`icons`) |
| `~/.local/state/odotui/history.json` | historial de restauraciones (las de la IA van marcadas) |
| `~/.local/state/odotui/odoo-<perfil>.{pid,log}` | el Odoo local en segundo plano |

## 🛠️ Desarrollo

```bash
cargo test                      # pruebas unitarias
cargo clippy --all-targets      # sin avisos
cargo install --path . --locked # actualiza el binario
```

La interfaz sigue el patrón de [tuisentry](https://github.com/jorge07RD/tuisentry): cada clic, tecla o comando de la paleta es un `Action` que pasa por `App::apply`. Las áreas clicables se registran en un `HitMap` que se rehace en cada frame.
