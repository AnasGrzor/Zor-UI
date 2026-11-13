//! The one list of Settings pages. Everything else reads from here: the nav and its order, the category headers,
//! the page titles and blurbs, the saved page, and the search labels.
//!
//! To add a page:
//! 1. add a variant to `Page` and to `Page::ALL`,
//! 2. add a row to `PAGES` (its category decides where it sits in the nav),
//! 3. add its arm in `Settings::body` (pages.rs). The compiler stops the build until that arm exists.
//!
//! The tests at the bottom fail when a row is missing, duplicated, or has no title or blurb.

/// A settings page. `body` matches on this with no catch-all arm, so a new page cannot be left undrawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page { Quick, Wallpapers, Presets, Bar, Island, Sidebar, Keys, Buddies, Contacts, Creator, Voice, Services, Usage, About }

impl Page {
    pub const ALL: [Page; 14] = [
        Page::Quick, Page::Wallpapers, Page::Presets, Page::Bar, Page::Island, Page::Sidebar, Page::Keys,
        Page::Buddies, Page::Contacts, Page::Creator, Page::Voice, Page::Services, Page::Usage, Page::About,
    ];
}

/// A nav group. The nav shows the groups in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category { Personalization, Interface, BuddiesVoice, System }

pub struct CategoryDef {
    pub cat: Category,
    pub name: &'static str,
}

pub const CATEGORIES: [CategoryDef; 4] = [
    CategoryDef { cat: Category::Personalization, name: "Personalization" },
    CategoryDef { cat: Category::Interface, name: "Interface" },
    CategoryDef { cat: Category::BuddiesVoice, name: "Buddies & Voice" },
    CategoryDef { cat: Category::System, name: "System" },
];

pub struct PageDef {
    pub page: Page,
    /// The saved-page and deep-link key. Never change it once shipped.
    pub key: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    /// One line under the page title.
    pub blurb: &'static str,
    pub cat: Category,
}

/// Every page, in the order they appear inside their category.
pub const PAGES: [PageDef; 14] = [
    PageDef { page: Page::Quick, key: "quick", icon: "tune", title: "Quick", blurb: "Wallpaper, colors and the switches you reach for most", cat: Category::Personalization },
    PageDef { page: Page::Wallpapers, key: "wallpapers", icon: "animated_images", title: "Wallpapers", blurb: "Pick a picture or a live wallpaper", cat: Category::Personalization },
    PageDef { page: Page::Presets, key: "presets", icon: "palette", title: "Presets", blurb: "Save and swap whole looks", cat: Category::Personalization },
    PageDef { page: Page::Bar, key: "bar", icon: "toolbar", title: "Bar & desktop", blurb: "The bar, the desktop cards and how they are laid out", cat: Category::Interface },
    PageDef { page: Page::Island, key: "island", icon: "pill", title: "Dynamic Island", blurb: "What the island shows and how it reacts", cat: Category::Interface },
    PageDef { page: Page::Sidebar, key: "sidebar", icon: "view_sidebar", title: "Left sidebar", blurb: "Translator, anime and football in the left sidebar", cat: Category::Interface },
    PageDef { page: Page::Keys, key: "keys", icon: "keyboard", title: "Keybinds", blurb: "Every keyboard shortcut", cat: Category::Interface },
    PageDef { page: Page::Buddies, key: "buddies", icon: "pets", title: "Buddies", blurb: "The characters that live on your desktop", cat: Category::BuddiesVoice },
    PageDef { page: Page::Contacts, key: "contacts", icon: "contacts", title: "Contacts", blurb: "The people the buddy can message by voice", cat: Category::BuddiesVoice },
    PageDef { page: Page::Creator, key: "creator", icon: "person_add", title: "Create a buddy", blurb: "Turn a picture into a buddy that looks like your character", cat: Category::BuddiesVoice },
    PageDef { page: Page::Voice, key: "voice", icon: "mic", title: "Voice", blurb: "Talk to the buddy, what it may do for you, and the speech model", cat: Category::BuddiesVoice },
    PageDef { page: Page::Services, key: "services", icon: "hub", title: "Services", blurb: "API keys, models and sign-ins", cat: Category::System },
    PageDef { page: Page::Usage, key: "usage", icon: "memory", title: "Usage", blurb: "What the AI tools have used", cat: Category::System },
    PageDef { page: Page::About, key: "about", icon: "info", title: "About", blurb: "Version and updates", cat: Category::System },
];

/// The row of `PAGES` for `page`.
pub fn def_of(page: Page) -> &'static PageDef {
    PAGES.iter().find(|d| d.page == page).expect("every Page has a row in PAGES")
}

/// The index into `PAGES` of `page`. Used for `Act::Page` and `Settings::page`.
pub fn index_of(page: Page) -> usize {
    PAGES.iter().position(|d| d.page == page).expect("every Page has a row in PAGES")
}

/// The index into `PAGES` of the page with `key`, if there is one.
pub fn index_of_key(key: &str) -> Option<usize> {
    PAGES.iter().position(|d| d.key == key)
}

/// The category's display name.
pub fn category_name(cat: Category) -> &'static str {
    CATEGORIES.iter().find(|c| c.cat == cat).map_or("General", |c| c.name)
}

/// The `PAGES` indexes in nav order: grouped by category, categories in `CATEGORIES` order.
/// Adding a page never needs its position in the list to be right; the group sets it.
pub fn nav_order() -> Vec<usize> {
    CATEGORIES.iter().flat_map(|c| PAGES.iter().enumerate().filter(move |(_, d)| d.cat == c.cat).map(|(i, _)| i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_has_exactly_one_row() {
        for page in Page::ALL {
            assert_eq!(PAGES.iter().filter(|d| d.page == page).count(), 1, "{page:?}");
        }
        assert_eq!(PAGES.len(), Page::ALL.len());
    }

    #[test]
    fn keys_are_unique_and_every_row_has_its_text() {
        let mut keys: Vec<&str> = PAGES.iter().map(|d| d.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), PAGES.len(), "duplicate page key");
        for d in &PAGES {
            assert!(!d.title.is_empty() && !d.blurb.is_empty() && !d.icon.is_empty(), "{}", d.key);
            assert_ne!(category_name(d.cat), "General", "{} has no category", d.key);
        }
    }

    #[test]
    fn nav_lists_every_page_once_grouped_by_category() {
        let order = nav_order();
        assert_eq!(order.len(), PAGES.len());
        let mut seen = order.clone();
        seen.sort();
        assert_eq!(seen, (0..PAGES.len()).collect::<Vec<_>>());
        let rank: Vec<usize> = order.iter().map(|&i| CATEGORIES.iter().position(|c| c.cat == PAGES[i].cat).unwrap()).collect();
        assert!(rank.windows(2).all(|w| w[0] <= w[1]), "categories are not grouped: {rank:?}");
    }

    #[test]
    fn the_first_page_is_quick_and_keys_round_trip() {
        assert_eq!(PAGES[0].key, "quick");
        for (i, d) in PAGES.iter().enumerate() {
            assert_eq!(index_of(d.page), i);
            assert_eq!(index_of_key(d.key), Some(i));
        }
        assert_eq!(index_of_key("nope"), None);
    }
}
