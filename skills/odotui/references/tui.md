# La TUI de odotui: pestañas, teclas y mouse

Para guiar a Jorge cuando prefiere hacerlo a mano. Todo se puede hacer con clics:
pestañas, filas (el segundo clic abre la fila), casillas, botones (cada uno muestra su tecla),
rueda y arrastre.

## Globales

| Tecla | Acción |
|---|---|
| `Tab` · `1`-`5` | Cambiar de pestaña |
| `p` · clic en el perfil ▾ | Siguiente perfil |
| `↑↓` `jk` · `PgUp` `PgDn` · `Home` `End` | Moverse |
| `←→` `hl` | Cambiar de panel |
| `Enter` | Abrir la fila seleccionada |
| `/` · `Esc` | Filtrar (ignora tildes) · borrar el filtro o cerrar |
| `:` · `≡` | Paleta de comandos |
| `f` | Seguir el final del log |
| `e` | Ver solo los errores del log |
| `y` · clic en una línea roja | Copiar el último error (o ese) con su traceback |
| `C` | Limpiar el log |
| `X` | Cancelar la tarea en curso |
| `?` · `q` | Ayuda · salir |

## 1 BDs

| Tecla | Acción |
|---|---|
| `r` | Refrescar |
| `m` · `Enter` | Ver los módulos de la BD |
| `s` | Restaurar un backup sobre esta BD |
| `b` | Backup (zip o dump) |
| `c` · `n` | Duplicar · renombrar |
| `x` | Neutralizar (pide la contraseña del administrador) |
| `d` | Borrar (con confirmación) |
| `S` · `P` | `odoo shell` · `psql` interactivos; la TUI se suspende y vuelve al salir |

## 2 Restaurar

| Tecla | Acción |
|---|---|
| `R` · `Enter` | Restaurar el backup seleccionado (formulario: BD destino, sobrescribir, filestore, neutralizar, contraseña) |
| `r` | Refrescar la lista |

La barra muestra el avance real. Abajo está el historial (las restauraciones de la IA van marcadas).

## 3 Instancia

| Tecla | Acción |
|---|---|
| `s` · `t` · `R` | Iniciar · parar · reiniciar Odoo |
| `L` | Seguir o dejar el log |
| `v` | `--dev=all` (solo local, al próximo inicio) |
| `o` | Abrir Odoo en el navegador |

El log se colorea por partes: nivel, BD, logger, peticiones web (método, ruta, estado, tiempo)
y tracebacks con una banda roja.

## 4 Módulos

| Tecla | Acción |
|---|---|
| `b` · `B` | BD siguiente · anterior |
| `Espacio` · clic en `[ ]` | Marcar |
| `i` · `u` | Instalar · actualizar los marcados (o el seleccionado) |
| `U` | Actualizar los que tienen el código cambiado (⟳) |
| `d` | Desinstalar (con confirmación) |
| `x` | Desmarcar todo |

## 5 Perfiles

| Tecla | Acción |
|---|---|
| `a` · `e` · `d` | Nuevo · editar · borrar |
| `Enter` | Usar este perfil |
| `t` | Probar la conexión (Postgres, contenedores, rutas) |
| `*` | Marcar como perfil por defecto |
