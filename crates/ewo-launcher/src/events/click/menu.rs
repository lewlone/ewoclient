//! Main-menu clicks: the sidebar items and the network (lobby) widget.

use super::super::hit::lobby_join_target;
use super::*;

/// Action triggered by clicking a sidebar menu item on the main menu.
#[derive(Copy, Clone, Debug)]
enum MenuAction {
    Navigate(Screen),
    About,
    Quit,
}

const MAIN_MENU_ACTIONS: [MenuAction; 4] = [
    MenuAction::Navigate(Screen::Instances),
    MenuAction::Navigate(Screen::Settings),
    MenuAction::About,
    MenuAction::Quit,
];

impl App {
    /// Step 2: the main-menu sidebar items, then the network widget. Returns
    /// whether the press landed on one of them.
    pub(super) fn click_main_menu(&mut self, event_loop: &ActiveEventLoop, c: Click) -> bool {
        if self.screen != Screen::MainMenu {
            return false;
        }
        let mut handled = false;
        let bounds = self
            .fonts
            .as_ref()
            .map(|fonts| screens::main_menu::menu_item_bounds(c.card_w, c.card_h, fonts));
        if let Some(bounds) = bounds {
            for (i, b) in bounds.iter().enumerate() {
                let clicked = self.menu_items[i].update(c.pos, *b, c.pressed, c.time);
                if clicked {
                    self.run_menu_action(event_loop, MAIN_MENU_ACTIONS[i]);
                }
                if self.menu_items[i].hover && c.pressed {
                    handled = true;
                }
            }
        }
        if !handled {
            handled = self.click_server_widget(c);
        }
        handled
    }

    fn run_menu_action(&mut self, event_loop: &ActiveEventLoop, action: MenuAction) {
        match action {
            MenuAction::Navigate(target) => {
                log::info!("nav (menu): {:?} → {:?}", self.screen, target);
                self.screen = target;
                if target == Screen::Friends {
                    self.refresh_friends_on_arrival();
                }
            }
            MenuAction::About => {
                log::info!("about: clicked → opening About modal");
                self.about_modal.open();
            }
            MenuAction::Quit => {
                log::info!("quit: closing app");
                event_loop.exit();
            }
        }
    }

    /// H6: clicking the network widget joins the Frogsy lobby (only when the
    /// widget is shown, the network reports online, and a lobby address is
    /// configured). The click is claimed whenever it lands on the widget.
    fn click_server_widget(&mut self, c: Click) -> bool {
        if !self.server_widget_visible() {
            return false;
        }
        let server_rect = self.server_widget_rect(c.card_w, c.card_h);
        if !rect_contains(&server_rect, c.pos) {
            return false;
        }
        let online = matches!(self.social.server_status(), Some(s) if s.online);
        let lobby = self.social.lobby_addr().map(str::to_string);
        if let Some(lobby) = lobby_join_target(online, lobby.as_deref()) {
            log::info!("h6: network widget clicked → joining lobby");
            self.start_launch(self.instance_prefs.selected, Some(lobby), c.time);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MAIN_MENU_ACTIONS[i]` is the action of the i-th sidebar item
    /// `menu_item_bounds` lays out: Instances, Settings, About, Quit.
    #[test]
    fn menu_actions_follow_the_sidebar_order() {
        assert!(matches!(
            MAIN_MENU_ACTIONS[0],
            MenuAction::Navigate(Screen::Instances)
        ));
        assert!(matches!(
            MAIN_MENU_ACTIONS[1],
            MenuAction::Navigate(Screen::Settings)
        ));
        assert!(matches!(MAIN_MENU_ACTIONS[2], MenuAction::About));
        assert!(matches!(MAIN_MENU_ACTIONS[3], MenuAction::Quit));
    }
}
