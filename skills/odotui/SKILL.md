---
name: odotui
description: "Maneja entornos de desarrollo de Odoo con odotui (TUI + servidor MCP en ~/Projects/odotui): bases de datos, backups, restaurar y neutralizar, servidor de Odoo, módulos, tests, logs y el modelo de datos. Úsalo SIEMPRE que se pida algo de Odoo en local o docker — «restaura el backup», «actualiza el módulo», «corre los tests», «qué error da Odoo», «revisa el log», «qué campos tiene sale.order», «crea un módulo», «duplica la BD», «no puedo entrar con admin» — o se mencione odotui, aunque no se nombre la herramienta."
---

# odotui

odotui es la TUI de Jorge para desarrollar con Odoo, y además un servidor MCP. Con él la IA
hace lo mismo que la TUI. **Si la TUI está abierta, cada llamada corre dentro de ella** y
Jorge lo ve en vivo: la pestaña, la barra de progreso, el log y `◉ IA` en la cabecera.

| Qué | Dónde |
|---|---|
| Código | `~/Projects/odotui` (Rust) |
| Config | `~/.config/odotui/config.toml` (perfiles docker / local; también desde la pestaña Perfiles) |
| Binario | `~/.cargo/bin/odotui` · servidor MCP: `odotui mcp` |
| Estado | `~/.local/state/odotui/` (historial, `odoo-<perfil>.{pid,log}` del Odoo local) |
| Odoo 17 de prueba | `~/Projects/odotui/dev/docker-compose.yml` (perfil `odoo17-dev`) |

## Cómo llamar a las herramientas

En Claude Code aparecen como `mcp__odotui__<herramienta>`. Si no están cargadas:
- **Busca primero** con ToolSearch (`odotui`).
- Si no existen, el servidor no está registrado. Pídele a Jorge permiso para registrarlo:
  `claude mcp add -s user odotui -- ~/.cargo/bin/odotui mcp`. Las herramientas aparecen en la próxima sesión.
- Mientras tanto, usa `scripts/llamar.py` de este skill, que habla el mismo protocolo por stdio:
  ```bash
  python3 ~/.claude/skills/odotui/scripts/llamar.py status
  python3 ~/.claude/skills/odotui/scripts/llamar.py list_modules '{"database": "prueba", "state": "installed"}'
  ```

## Empieza siempre con `status`

Devuelve el perfil activo (docker o local), sus contenedores y rutas, el estado de Odoo, la
tarea en curso y si la TUI está abierta. No asumas nada de eso: cambia según el perfil.

## Ciclo al desarrollar un módulo

1. **Entender**:
   - `describe_model`: campos y el módulo que define cada uno.
   - `find_views` / `get_view`: vistas y su herencia.
   - `module_info`: manifest, dependencias, si el código cambió sin actualizar y licencias inválidas.
   - `search_records`: datos a través del ORM.
2. **Editar** con tus herramientas de archivos, en `addons_host_dir`: es la carpeta de
   addons vista desde este equipo. `addons_dir` es la misma carpeta vista por Odoo; en docker es una ruta del contenedor.
3. **Actualizar**:
   - `update_modules`, o `install_modules` si el módulo es nuevo.
   - Python cambiado → `instance` con `restart`. En local con `--dev=all` se recarga solo.
   - XML, datos o campos nuevos → siempre `update_modules`.
4. **Testear**: `run_tests`. Si hay fallos devuelve **error** con el resumen y cada traceback.
   Para más contexto: `read_logs` con `errors_only`, pasando el `since` que devolvió la lectura anterior.

Usa `show` para llevar a Jorge a la pestaña relevante cuando la TUI esté abierta.

## Reglas

- **Irreversible: confírmalo con Jorge antes.** Esto incluye `drop_database`,
  `restore_backup` con `overwrite`, `uninstall_modules`, `sql_query` con `write=true` y
  cualquier `odoo_shell` que haga `env.cr.commit()`.
  Antes de algo arriesgado sobre una BD con datos, `backup_database`.
- **Una tarea larga a la vez**, de Jorge o tuya. Si la respuesta es «ocupado», espera o pregunta
  antes de `cancel`: puede ser una restauración de Jorge.
- **`sql_query` es de solo lectura** salvo que pases `write=true`. Para leer, prefiere SQL o
  `search_records`. Para lógica de negocio, `odoo_shell` (hace rollback salvo `env.cr.commit()`; usa `print()`).
- **No uses `sudo`** ni cambies grupos o permisos por tu cuenta.

## Problemas conocidos (ya vistos con Odoo real)

| Síntoma | Causa | Qué hacer |
|---|---|---|
| `permission denied … docker.sock` | El proceso no tiene el grupo `docker` | Con la TUI abierta las llamadas usan sus permisos. Si está cerrada, Jorge debe volver a iniciar sesión, o abrir odotui desde una terminal con `newgrp docker` |
| «Usuario o contraseña incorrectos» con admin | En BDs creadas desde el gestor web el administrador tiene como login **el email** que se puso | `sql_query`: `SELECT login FROM res_users WHERE id = 2`. `neutralize_database` pone la contraseña y su log dice el login exacto |
| `Wrong value for ir.module.module.license` al actualizar | Licencia que Odoo no acepta (p. ej. `MIT`) | Usa `LGPL-3`, `AGPL-3`, `OPL-1`… `module_info` y el log lo avisan antes |
| `Address already in use` en tests | Odoo abre HTTP con `--test-enable` | odotui ya usa un puerto propio; si reaparece, revisa la versión instalada |
| Una herramienta no existe | Versión vieja instalada | `cd ~/Projects/odotui && cargo install --path . --locked` (pregunta antes) |

## Más detalle

- `references/herramientas.md`: las 27 herramientas con sus argumentos.
- `references/tui.md`: pestañas, teclas y mouse, para guiar a Jorge cuando prefiera hacerlo a mano.
