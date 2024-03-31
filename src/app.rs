// SPDX-FileCopyrightText: © 2024 David Bliss
//
// SPDX-License-Identifier: GPL-3.0-or-later

use relm4::{
    actions::{RelmAction, RelmActionGroup},
    adw, gtk, main_application, Component, ComponentController, ComponentParts, ComponentSender,
    Controller, SimpleComponent, WorkerController,
};

use gtk::prelude::{
    ApplicationExt, ApplicationWindowExt, GtkWindowExt, OrientableExt, SettingsExt, WidgetExt,
};
use gtk::{gio, glib};
use relm4::adw::prelude::AdwApplicationWindowExt;

use crate::config::{APP_ID, PROFILE};
use photos_core::repo::PictureId;
use photos_core::YearMonth;
use relm4::adw::prelude::NavigationPageExt;
use std::sync::{Arc, Mutex};

mod components;

use self::components::{
    about::AboutDialog, all_photos::AllPhotos, all_photos::AllPhotosInput,
    all_photos::AllPhotosOutput, month_photos::MonthPhotos, month_photos::MonthPhotosInput,
    month_photos::MonthPhotosOutput, one_photo::OnePhoto, one_photo::OnePhotoInput,
    year_photos::YearPhotos, year_photos::YearPhotosOutput,
};

mod background;

use self::background::{
    scan_photos::ScanPhotos,
    scan_photos::ScanPhotosInput,
    scan_photos::ScanPhotosOutput,
    generate_previews::GeneratePreviews,
    generate_previews::GeneratePreviewsInput,
    generate_previews::GeneratePreviewsOutput,
};

pub(super) struct App {
    scan_photos: WorkerController<ScanPhotos>,
    generate_previews: WorkerController<GeneratePreviews>,
    about_dialog: Controller<AboutDialog>,
    all_photos: Controller<AllPhotos>,
    month_photos: Controller<MonthPhotos>,
    year_photos: Controller<YearPhotos>,
    one_photo: Controller<OnePhoto>,

    // Library pages
    view_stack: adw::ViewStack,

    // Switch between library views and single image view.
    picture_navigation_view: adw::NavigationView,
}

#[derive(Debug)]
pub(super) enum AppMsg {
    Quit,

    // Show photo for ID.
    ViewPhoto(PictureId),

    // Scroll to first photo in month
    GoToMonth(YearMonth),

    // Scroll to first photo in year
    GoToYear(i32),

    // Photos have been scanned and repo can be updated
    ScanAllCompleted,

    // Preview generation completed
    PreviewsGenerated,
}

relm4::new_action_group!(pub(super) WindowActionGroup, "win");
relm4::new_stateless_action!(PreferencesAction, WindowActionGroup, "preferences");
relm4::new_stateless_action!(pub(super) ShortcutsAction, WindowActionGroup, "show-help-overlay");
relm4::new_stateless_action!(AboutAction, WindowActionGroup, "about");

#[relm4::component(pub)]
impl SimpleComponent for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();
    type Widgets = AppWidgets;

    menu! {
        primary_menu: {
            section! {
                "_Preferences" => PreferencesAction,
                "_Keyboard" => ShortcutsAction,
                "_About Photo Romantic" => AboutAction,
            }
        }
    }

    view! {
        main_window = adw::ApplicationWindow::new(&main_application()) {
            set_visible: true,
            set_width_request: 400,
            set_height_request: 400,

            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::Quit);
                glib::Propagation::Stop
            },

            #[wrap(Some)]
            set_help_overlay: shortcuts = &gtk::Builder::from_resource(
                    "/dev/romantics/Photos/gtk/help-overlay.ui"
                )
                .object::<gtk::ShortcutsWindow>("help_overlay")
                .unwrap() -> gtk::ShortcutsWindow {
                    set_transient_for: Some(&main_window),
                    set_application: Some(&main_application()),
            },

            add_css_class?: if PROFILE == "Devel" {
                    Some("devel")
                } else {
                    None
                },


            add_breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MaxWidth,
                500.0,
                adw::LengthUnit::Sp,
            )) {
                add_setter: (&header_bar, "show-title", &false.into()),
                add_setter: (&switcher_bar, "reveal", &true.into()),
            },

            #[local_ref]
            picture_navigation_view -> adw::NavigationView {
                set_pop_on_escape: true,

                adw::NavigationPage {
                    set_tag: Some("time_period_views"),
                    adw::ToolbarView {

                        #[name = "header_bar"]
                        add_top_bar = &adw::HeaderBar {
                            set_hexpand: true,

                            #[wrap(Some)]
                            set_title_widget = &adw::ViewSwitcher {
                                set_stack: Some(&view_stack),
                                set_policy: adw::ViewSwitcherPolicy::Wide,
                            },

                            pack_end = &gtk::MenuButton {
                                set_icon_name: "open-menu-symbolic",
                                set_menu_model: Some(&primary_menu),
                            }
                        },

                        #[wrap(Some)]
                        set_content = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            #[local_ref]
                            view_stack -> adw::ViewStack {
                                add_titled_with_icon[Some("all"), "All", "playlist-infinite-symbolic"] = model.all_photos.widget(),
                                add_titled_with_icon[Some("month"), "Month", "month-symbolic"] = model.month_photos.widget(),
                                add_titled_with_icon[Some("year"), "Year", "year-symbolic"] = model.year_photos.widget(),
                            },

                            #[name(switcher_bar)]
                            adw::ViewSwitcherBar {
                                set_stack: Some(&view_stack),
                            },
                        },
                    },
                },

                adw::NavigationPage {
                    set_tag: Some("picture"),

                    // one_photo is the full-screen display of a given photo
                    model.one_photo.widget(),
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let data_dir = glib::user_data_dir().join("photo-romantic");
        let _ = std::fs::create_dir_all(&data_dir);

        let cache_dir = glib::user_cache_dir().join("photo-romantic");
        let _ = std::fs::create_dir_all(&cache_dir);

        // TODO use XDG_PICTURES_DIR as the default, but let users override in preferences.
        let pic_base_dir = glib::user_special_dir(glib::enums::UserDirectory::Pictures)
            .expect("Expect XDG_PICTURES_DIR");

        let repo = {
            let db_path = data_dir.join("pictures.sqlite");
            photos_core::Repository::open(&pic_base_dir, &db_path).unwrap()
        };

        let scan = photos_core::Scanner::build(&pic_base_dir).unwrap();

        let previewer = {
            let preview_base_path = cache_dir.join("previews");
            let _ = std::fs::create_dir_all(&preview_base_path);
            photos_core::Previewer::build(&preview_base_path).unwrap()
        };

        let repo = Arc::new(Mutex::new(repo));

        //let controller = photos_core::Controller::new(scan.clone(), repo, previewer);
        //let controller = Arc::new(Mutex::new(controller));

        let scan_photos = ScanPhotos::builder()
            .detach_worker((scan.clone(), repo.clone()))
            .forward(sender.input_sender(), |msg| match msg {
                ScanPhotosOutput::ScanAllCompleted => AppMsg::ScanAllCompleted,
            });

        let generate_previews = GeneratePreviews::builder()
            .detach_worker((scan.clone(), previewer.clone(), repo.clone()))
            .forward(sender.input_sender(), |msg| match msg {
                GeneratePreviewsOutput::PreviewsGenerated => AppMsg::PreviewsGenerated,
            });

        let all_photos = AllPhotos::builder()
            .launch(repo.clone())
            .forward(sender.input_sender(), |msg| match msg {
                AllPhotosOutput::PhotoSelected(id) => AppMsg::ViewPhoto(id),
            });

        let month_photos = MonthPhotos::builder()
            .launch(repo.clone())
            .forward(sender.input_sender(), |msg| match msg {
                MonthPhotosOutput::MonthSelected(ym) => AppMsg::GoToMonth(ym),
            });

        let year_photos = YearPhotos::builder()
            .launch(repo.clone())
            .forward(sender.input_sender(), |msg| match msg {
                YearPhotosOutput::YearSelected(year) => AppMsg::GoToYear(year),
            });

        let one_photo = OnePhoto::builder()
            .launch(repo.clone())
            .detach();

        let about_dialog = AboutDialog::builder()
            .transient_for(&root)
            .launch(())
            .detach();

        let view_stack = adw::ViewStack::new();

        let picture_navigation_view = adw::NavigationView::builder().build();

        let model = Self {
            scan_photos,
            generate_previews,
            about_dialog,
            all_photos,
            month_photos,
            year_photos,
            one_photo,
            view_stack: view_stack.clone(),
            picture_navigation_view: picture_navigation_view.clone(),
        };

        let widgets = view_output!();

        let mut actions = RelmActionGroup::<WindowActionGroup>::new();

        let shortcuts_action = {
            let shortcuts = widgets.shortcuts.clone();
            RelmAction::<ShortcutsAction>::new_stateless(move |_| {
                shortcuts.present();
            })
        };

        let about_action = {
            let sender = model.about_dialog.sender().clone();
            RelmAction::<AboutAction>::new_stateless(move |_| {
                sender.send(()).unwrap();
            })
        };

        actions.add_action(shortcuts_action);
        actions.add_action(about_action);
        actions.register_for_widget(&widgets.main_window);

        widgets.load_window_size();

        model.scan_photos.sender().emit(ScanPhotosInput::ScanAll);

        ComponentParts { model, widgets }
    }

    fn update(&mut self, message: Self::Input, _sender: ComponentSender<Self>) {
        match message {
            AppMsg::Quit => main_application().quit(),
            AppMsg::ViewPhoto(picture_id) => {
                // Send message to OnePhoto to show image
                self.one_photo.emit(OnePhotoInput::ViewPhoto(picture_id));

                // Display navigation page for viewing an individual photo.
                self.picture_navigation_view.push_by_tag("picture");
            },
            AppMsg::GoToMonth(ym) => {
                // Display all photos view.
                self.view_stack.set_visible_child_name("all");
                self.all_photos.emit(AllPhotosInput::GoToMonth(ym));
            },
            AppMsg::GoToYear(year) => {
                // Display month photos view.
                self.view_stack.set_visible_child_name("month");
                self.month_photos.emit(MonthPhotosInput::GoToYear(year));
            },
            AppMsg::ScanAllCompleted => {
                println!("Scan all completed msg received.");
                self.generate_previews.emit(GeneratePreviewsInput::Generate);
            },
            AppMsg::PreviewsGenerated => {
                println!("Previews generated completed.");
            },
        }
    }

    fn shutdown(&mut self, widgets: &mut Self::Widgets, _output: relm4::Sender<Self::Output>) {
        widgets.save_window_size().unwrap();
    }
}

impl AppWidgets {
    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let settings = gio::Settings::new(APP_ID);
        let (width, height) = self.main_window.default_size();

        settings.set_int("window-width", width)?;
        settings.set_int("window-height", height)?;

        settings.set_boolean("is-maximized", self.main_window.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let settings = gio::Settings::new(APP_ID);

        let width = settings.int("window-width");
        let height = settings.int("window-height");
        let is_maximized = settings.boolean("is-maximized");

        self.main_window.set_default_size(width, height);

        if is_maximized {
            self.main_window.maximize();
        }
    }
}
