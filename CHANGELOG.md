# Cambios

Formato de [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/); las versiones siguen [SemVer](https://semver.org/lang/es/).

## [0.2.0] - 2026-10-31

### Añadido

- **Backups**: crear backups desde BDs (`b`) o por MCP (`backup_database`): `.zip` compatible con el gestor de Odoo (`dump.sql`, `filestore/` y `manifest.json`) o `.dump` de `pg_dump -Fc`. Muestra los MB escritos y la compresión; si falla o se cancela no queda nada a medias.
- **Restaurar más formatos**: `.zip` del gestor de Odoo / Odoo.sh (el `dump.sql` se lee del zip sin extraerlo), `.dump` / `.backup` (`pg_restore`), `.sql` y `.sql.gz`. La lista muestra el tipo y la versión de Odoo del manifest, y el formulario avisa si no coincide con la del perfil.
- **Addons con cambios**: con `addons_host_dir` (la carpeta de addons en el host), Módulos marca con ⟳ lo instalado cuyo `__manifest__.py` tiene otra versión; `U` lo actualiza. Los addons propios que la BD aún no lista también aparecen.
- **odoo shell y psql interactivos** sobre la BD seleccionada (`S` y `P` en BDs): la TUI se suspende y vuelve intacta al salir.
- **Desinstalar módulos** (`d` en Módulos, con confirmación) y la herramienta MCP `uninstall_modules`.
- **Tracebacks**: `y` copia el último error del log, `e` muestra solo errores y un clic en una línea roja copia su traceback (`wl-copy` u OSC 52).
- **Aviso al terminar**: las tareas de más de 20 s mandan un `notify-send`.
- **IA**: `describe_model`, `find_views`, `get_view`, `module_info` y `search_records` para entender el modelo de datos sin escribir SQL; `read_logs` con `since` para leer solo lo nuevo; avance de restauraciones y backups como `notifications/progress` si la llamada trae `_meta.progressToken`.
- **Log de Odoo con colores por partes**: hora, nivel (ERROR como insignia roja, WARN en amarillo), BD, logger y mensaje; las peticiones web compactadas en método, ruta, estado HTTP coloreado, consultas y tiempo (las de 1 s o más resaltan); los tracebacks colorean archivo, línea, función y excepción, y la banda roja cubre el error entero.
- **Aviso de licencias**: antes de instalar, actualizar o testear se avisa si un addon propio declara una licencia que Odoo no acepta (falla al actualizar con un error críptico); `module_info` también lo muestra.
- **Calidad**: CI (fmt, clippy, tests y e2e contra Postgres 16), tests de la interfaz con `TestBackend`, `dev/docker-compose.yml` con Odoo 17 real y `scripts/mcp_smoke.py`, que recorre todas las herramientas MCP contra él. Releases con binarios para Linux (gnu y musl). Licencia MIT.

### Cambiado

- El botón «Restaurar aquí» de BDs ahora se llama «Restaurar».
- Validado contra Odoo 17 real (`scripts/mcp_smoke.py`: 36 ✓). La CI usa las versiones actuales de las acciones de GitHub (Node 24).
- Los errores inocuos de `transaction_timeout` (un `pg_dump` más nuevo que el servidor) no cuentan como errores al restaurar.

### Corregido

- **Neutralizar** cambia la contraseña del administrador real (`base.user_admin`). En las BDs creadas desde el gestor web su login es el email que se puso, no «admin», y antes no se cambiaba nada. El log dice con qué login entrar.
- **`run_tests`** usa un puerto HTTP propio: con `--test-enable` Odoo levanta su servidor web aunque se pida `--no-http`, y chocaba con el 8069 de la instancia en marcha.

## [0.1.0] - 2026-10-31

Primera versión: BDs (listar, duplicar, renombrar, neutralizar, borrar), restaurar carpetas `dump.sql` + `filestore/` con progreso, instancia de Odoo (docker o `odoo-bin` local) con el log en vivo, módulos (`-i` / `-u`), perfiles, paleta de comandos, mouse en todo y el servidor MCP `odotui mcp` con 20 herramientas.

[0.2.0]: https://github.com/jorge07RD/odotui/releases/tag/v0.2.0
[0.1.0]: https://github.com/jorge07RD/odotui/tree/b1b56ea
