# odotui

Herramientas de desarrollo para Odoo en la terminal, 100% usables con el mouse
**o** con el teclado. Escrito en Rust con ratatui; la interacción sigue el
patrón de `tuisentry` (un `Action` por cada clic o tecla y un mapa de áreas
clicables que se rehace en cada frame).

| Pestaña | Qué hace |
|---|---|
| **1 BDs** | Lista las bases con tamaño, dueño y versión de Odoo. Duplicar, renombrar, neutralizar y borrar, siempre junto con su filestore. |
| **2 Restaurar** | Carga `<backups_dir>/<carpeta>/dump.sql` (+ `filestore/`) con progreso real, cancelable, y con neutralización opcional. Guarda un historial. |
| **3 Instancia** | Iniciar, parar y reiniciar Odoo (contenedor o `odoo-bin`) y ver su log en vivo: colores por nivel, seguir el final y filtrar. |
| **4 Módulos** | Elegir una BD, marcar módulos e instalarlos (`-i`) o actualizarlos (`-u`) con `--stop-after-init`. |
| **5 Perfiles** | Perfiles docker o local: crear, editar, probar la conexión y marcar uno por defecto. |

## Instalar

```bash
cargo build --release
cp target/release/odotui ~/.local/bin/
odotui            # sin config abre el formulario de perfil
odotui --init     # o escribe un ejemplo en ~/.config/odotui/config.toml
```

Requisitos: `psql` en el PATH. En modo docker, además, `docker` sin `sudo`
(el usuario en el grupo `docker`).

## Perfiles

Ver `config.example.toml`.

- **docker**: `psql` corre dentro de `pg_container` (contra `127.0.0.1`). Odoo
  se maneja con `docker start/stop/restart` sobre `odoo_container`. Los módulos
  se actualizan con `docker exec <odoo_container> odoo --db_host … -u …`. El
  filestore se copia con `docker cp` y se le hace `chown odoo:odoo`.
- **local**: se usa `psql` del sistema contra `pg_host`, y Odoo es
  `python odoo-bin -c odoo.conf`. Parar Odoo le manda SIGINT, así que cierra
  limpio.

Los backups siempre se leen **en este equipo**; en docker el dump entra por la
entrada estándar de `docker exec -i`.

## Restauración

1. Valida el nombre (`letras, números, _ - .`). Si la BD existe, pide
   confirmación, cierra sus conexiones y hace `DROP`.
2. `CREATE DATABASE … ENCODING 'UTF8' TEMPLATE template0`.
3. `psql -q` con el dump por stdin. Sin `ON_ERROR_STOP`, igual que Odoo: los
   errores inocuos (extensiones, parámetros de otra versión de pg_dump) se
   cuentan y se ven en rojo en el log.
4. Copia `filestore/` a `<filestore_dir>/<bd>`.
5. Neutraliza: desactiva crons, servidores de correo y pagos, quita el código
   enterprise, regenera `database.uuid`, apunta `web.base.url` a `odoo_url`,
   marca `database.is_neutralized` y pone la contraseña de `admin` (hash
   `pbkdf2-sha512` compatible con passlib).

`X` o el botón **Cancelar** matan `psql` y avisan de que la BD quedó a medias.

## Mouse y teclado

| Mouse | Teclado |
|---|---|
| clic en una pestaña | `Tab`, `1`-`5` |
| clic en `◆ perfil ▾` | `p` |
| clic en una fila; 2º clic la abre | `↑↓`/`jk`, `Enter` |
| clic en `[ ]` de un módulo | `Espacio` |
| botones `tecla etiqueta` | la tecla que muestran |
| rueda o arrastre | `PgUp`/`PgDn`, `Home`/`End` |
| `⇣ siguiendo` | `f` |
| barra `/ filtro` | `/`, `Esc` borra |
| `≡` (paleta) | `:` |
| `? ayuda`, `q salir` | `?`, `q` |

## Archivos

- `~/.config/odotui/config.toml`: los perfiles.
- `~/.local/state/odotui/history.json`: el historial de restauraciones.
