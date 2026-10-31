# Herramientas MCP de odotui (v0.2)

Todas aceptan `profile` (opcional; omitido = el activo), salvo `status`, `switch_profile`,
`show` y `cancel`. `*` = obligatorio. Las de **escritura** corren una a la vez y, con la
TUI abierta, se ven en su pestaña.

## Estado y navegación

| Herramienta | Argumentos | Para qué |
|---|---|---|
| `status` | — | Perfil activo, rutas, contenedores, estado de Odoo, tarea en curso, si la TUI está abierta |
| `switch_profile` | `profile*` | Cambiar de perfil (falla si hay una tarea en curso) |
| `show` | `tab*`: `dbs` · `restore` · `instance` · `modules` · `profiles` | Mostrar una pestaña en la TUI |
| `cancel` | — | Cancelar la tarea larga en curso |

## Lectura

| Herramienta | Argumentos | Devuelve |
|---|---|---|
| `list_databases` | — | BDs con versión de Odoo (`—` = no es de Odoo), tamaño y dueño |
| `list_backups` | — | Backups en `backups_dir`: `.zip` de Odoo, `.dump`, `.sql(.gz)`, carpetas con `dump.sql` |
| `list_modules` | `database*`, `filter`, `state` | Módulos con estado, versión y si el código cambió (⟳) |
| `read_logs` | `lines` (200), `filter`, `errors_only`, `since` | Log del servidor. `errors_only` = cada error con su traceback. `since` = solo lo nuevo (lo devuelve cada lectura) |
| `describe_model` | `database*`, `model*` | Campos: tipo, relación, required, store, compute y módulo que lo define |
| `find_views` | `database*`, `model` o `xml_id` | Vistas con xml_id, tipo, prioridad y herencia |
| `get_view` | `database*`, `xml_id*` | `arch` de la vista y de las que la heredan |
| `module_info` | `name*`, `database` | Manifest, dependencias, estado, si el código cambió y licencia inválida |
| `search_records` | `database*`, `model*`, `domain`, `fields`, `limit` | `search_read` del ORM como JSON |
| `sql_query` | `database*`, `sql*`, `limit` (200), `write` | Tabla de psql. **Solo lectura salvo `write=true`** |

## Escritura

| Herramienta | Argumentos | Notas |
|---|---|---|
| `restore_backup` | `backup*`, `database*`, `overwrite`, `neutralize` (sí), `filestore` (sí), `admin_password` («admin») | Si la BD existe hace falta `overwrite=true` y **se borra antes**. Informa avance |
| `backup_database` | `database*`, `format` (`zip` · `dump`), `filestore` | Zip compatible con el gestor de Odoo, en `backups_dir` |
| `duplicate_database` | `source*`, `target*` | Con su filestore; la copia recibe otro `database.uuid` |
| `rename_database` | `source*`, `target*` | Con su filestore |
| `drop_database` | `database*` | **Irreversible**, con su filestore |
| `neutralize_database` | `database*`, `admin_password` («admin») | Sin crons, correo ni pagos; `web.base.url` local; contraseña del administrador real (su login sale en el log) |
| `install_modules` | `database*`, `modules*` | `-i … --stop-after-init`. Avisa de licencias inválidas |
| `update_modules` | `database*`, `modules*` | `-u …`; después `instance restart` si cambió Python |
| `uninstall_modules` | `database*`, `modules*` | **Irreversible** (borra sus datos) |
| `run_tests` | `database*`, `modules*`, `test_tags` | `-u --test-enable --test-tags` con puerto HTTP propio. Con fallos devuelve error + tracebacks |
| `odoo_shell` | `database*`, `code*` | Python con `env`. Rollback salvo `env.cr.commit()`; usa `print()` |
| `scaffold_module` | `name*` | `odoo scaffold` en `addons_dir`; devuelve la ruta |
| `instance` | `action*`: `start` · `stop` · `restart`, `dev` (sí, solo local) | Docker: el contenedor. Local: `odoo-bin` en segundo plano |

## Ejemplos

```json
{"name": "restore_backup", "arguments": {"backup": "acme_prod_2025-10-30.zip", "database": "acme_dev"}}
{"name": "search_records", "arguments": {"database": "acme_dev", "model": "res.partner", "domain": [["is_company", "=", true]], "fields": ["name", "vat"], "limit": 5}}
{"name": "odoo_shell", "arguments": {"database": "acme_dev", "code": "print(env['sale.order'].search_count([('state', '=', 'draft')]))"}}
{"name": "read_logs", "arguments": {"errors_only": true}}
```
