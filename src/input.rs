//! Todo lo que se puede hacer con un clic o una tecla, y el mapa de áreas
//! clicables que se reconstruye en cada frame (patrón de tuisentry).

use ratatui::layout::{Position, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Dbs,
    Restore,
    Instance,
    Modules,
    Profiles,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Dbs,
        Tab::Restore,
        Tab::Instance,
        Tab::Modules,
        Tab::Profiles,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Dbs => "BDs",
            Tab::Restore => "Restaurar",
            Tab::Instance => "Instancia",
            Tab::Modules => "Módulos",
            Tab::Profiles => "Perfiles",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Paneles de la pestaña; el primero recibe el foco al entrar.
    pub fn panes(self) -> &'static [Pane] {
        match self {
            Tab::Dbs => &[Pane::DbList, Pane::DbLog],
            Tab::Restore => &[Pane::Backups, Pane::RestoreLog],
            Tab::Instance => &[Pane::InstanceLog],
            Tab::Modules => &[Pane::ModList, Pane::ModLog],
            Tab::Profiles => &[Pane::Profiles],
        }
    }
}

/// Áreas con scroll y/o filas seleccionables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    DbList,
    DbLog,
    Backups,
    RestoreLog,
    InstanceLog,
    ModList,
    ModLog,
    Profiles,
    Form,
    Palette,
}

impl Pane {
    pub fn is_log(self) -> bool {
        matches!(
            self,
            Pane::DbLog | Pane::RestoreLog | Pane::InstanceLog | Pane::ModLog
        )
    }
}

/// Acciones de los botones de cada pestaña.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Refresh,
    Cancel,
    Follow,
    ClearLog,
    DbDrop,
    DbDuplicate,
    DbRename,
    DbNeutralize,
    DbRestoreHere,
    DbModules,
    Restore,
    InstStart,
    InstStop,
    InstRestart,
    InstLogs,
    InstDev,
    InstOpen,
    /// BD anterior (-1) o siguiente (1) en Módulos.
    ModDb(i8),
    ModToggle,
    ModInstall,
    ModUpdate,
    ModClear,
    ProfNew,
    ProfEdit,
    ProfDelete,
    ProfDefault,
    ProfTest,
    ProfUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Tab(Tab),
    NextProfile,
    UseProfile(usize),
    /// Clic en una fila: la selecciona; si ya lo estaba, la abre.
    Row(Pane, usize),
    /// Clic en la casilla de una fila.
    Toggle(Pane, usize),
    Focus(Pane),
    Filter(Pane),
    Cmd(Cmd),
    Help,
    Palette,
    Close,
    Quit,
    /// Área que absorbe clics sin hacer nada (cuerpo de un popup).
    Block,
    FormFocus(usize),
    FormPrev(usize),
    FormNext(usize),
    FormSave,
    FormCancel,
    ConfirmYes,
    ConfirmNo,
}

/// Se reconstruye en cada frame: cada widget interactivo registra su área.
#[derive(Debug, Default)]
pub struct HitMap {
    hits: Vec<(Rect, Action)>,
    /// (área, panel, scroll máximo)
    scrolls: Vec<(Rect, Pane, usize)>,
    /// Filas visibles de cada panel en el último frame (para seguir la selección).
    visible: Vec<(Pane, usize)>,
}

impl HitMap {
    pub fn push(&mut self, area: Rect, action: Action) {
        if !area.is_empty() {
            self.hits.push((area, action));
        }
    }

    pub fn scroll(&mut self, area: Rect, pane: Pane, max: usize, rows: usize) {
        self.visible.push((pane, rows));
        self.scrolls.push((area, pane, max));
    }

    /// Popup modal: lo de debajo deja de ser interactivo y tocar fuera cierra.
    pub fn modal(&mut self, screen: Rect, popup: Rect) {
        self.scrolls.clear();
        self.push(screen, Action::Close);
        self.push(popup, Action::Block);
    }

    /// Como `modal`, pero tocar fuera no hace nada (para no perder lo escrito).
    pub fn modal_strict(&mut self, screen: Rect) {
        self.scrolls.clear();
        self.push(screen, Action::Block);
    }

    /// Lo registrado al final queda encima.
    pub fn at(&self, x: u16, y: u16) -> Option<Action> {
        let p = Position { x, y };
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(p))
            .map(|(_, a)| *a)
    }

    pub fn scroll_at(&self, x: u16, y: u16) -> Option<(Pane, usize)> {
        let p = Position { x, y };
        self.scrolls
            .iter()
            .rev()
            .find(|(r, ..)| r.contains(p))
            .map(|&(_, i, m)| (i, m))
    }

    pub fn max_scroll(&self, pane: Pane) -> usize {
        self.scrolls
            .iter()
            .find(|(_, i, _)| *i == pane)
            .map_or(0, |&(.., m)| m)
    }

    pub fn visible(&self, pane: Pane) -> usize {
        self.visible
            .iter()
            .find(|(i, _)| *i == pane)
            .map_or(10, |&(_, n)| n.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topmost_hit_wins() {
        let mut h = HitMap::default();
        h.push(Rect::new(0, 0, 10, 10), Action::Row(Pane::DbList, 0));
        h.push(Rect::new(2, 2, 3, 1), Action::Toggle(Pane::DbList, 0));
        assert_eq!(h.at(3, 2), Some(Action::Toggle(Pane::DbList, 0)));
        assert_eq!(h.at(0, 0), Some(Action::Row(Pane::DbList, 0)));
        assert_eq!(h.at(20, 20), None);
    }

    #[test]
    fn modal_covers_everything_below() {
        let mut h = HitMap::default();
        h.push(Rect::new(0, 0, 10, 1), Action::Tab(Tab::Restore));
        h.scroll(Rect::new(0, 1, 10, 9), Pane::DbList, 5, 9);
        h.modal(Rect::new(0, 0, 80, 24), Rect::new(20, 5, 40, 10));
        h.push(Rect::new(22, 12, 8, 1), Action::ConfirmYes);
        assert_eq!(h.at(1, 0), Some(Action::Close));
        assert_eq!(h.at(30, 6), Some(Action::Block));
        assert_eq!(h.at(23, 12), Some(Action::ConfirmYes));
        assert_eq!(h.scroll_at(1, 3), None);
        // Las filas visibles se recuerdan aunque el modal tape el scroll.
        assert_eq!(h.visible(Pane::DbList), 9);
    }
}
