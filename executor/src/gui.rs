//! Contains all helper functions that creates styled widgets for game user interface.
//! However most of the styles are used from dark theme of fyrox-ui library so there
//! is not much.

use fyrox::core::pool::Handle;
use fyrox::gui::{
    check_box::CheckBoxBuilder, scroll_bar::ScrollBarBuilder, scroll_viewer::ScrollViewerBuilder,
    widget::WidgetBuilder, BuildContext, HorizontalAlignment, Orientation, Thickness, UiNode,
    VerticalAlignment,
};

pub struct ScrollBarData {
    pub min: f32,
    pub max: f32,
    pub value: f32,
    pub step: f32,
    pub row: usize,
    pub column: usize,
    pub margin: Thickness,
    pub show_value: bool,
    pub orientation: Orientation,
}

pub fn create_scroll_bar(ctx: &mut BuildContext, data: ScrollBarData) -> Handle<UiNode> {
    let mut wb = WidgetBuilder::new();
    match data.orientation {
        Orientation::Vertical => wb = wb.with_width(30.0),
        Orientation::Horizontal => wb = wb.with_height(30.0),
    }
    ScrollBarBuilder::new(
        wb.on_row(data.row)
            .on_column(data.column)
            .with_margin(data.margin),
    )
    .with_orientation(data.orientation)
    .show_value(data.show_value)
    .with_max(data.max)
    .with_min(data.min)
    .with_step(data.step)
    .with_value(data.value)
    .with_value_precision(1)
    .build(ctx)
}

pub fn create_check_box(
    ctx: &mut BuildContext,
    row: usize,
    column: usize,
    checked: bool,
) -> Handle<UiNode> {
    CheckBoxBuilder::new(
        WidgetBuilder::new()
            .with_margin(Thickness::uniform(2.0))
            .with_width(24.0)
            .with_height(24.0)
            .on_row(row)
            .on_column(column)
            .with_vertical_alignment(VerticalAlignment::Center)
            .with_horizontal_alignment(HorizontalAlignment::Left),
    )
    .checked(Some(checked))
    .build(ctx)
}

pub fn create_scroll_viewer(ctx: &mut BuildContext) -> Handle<UiNode> {
    ScrollViewerBuilder::new(WidgetBuilder::new())
        .with_horizontal_scroll_bar(create_scroll_bar(
            ctx,
            ScrollBarData {
                min: 0.0,
                max: 0.0,
                value: 0.0,
                step: 0.0,
                row: 0,
                column: 0,
                margin: Default::default(),
                show_value: false,
                orientation: Orientation::Horizontal,
            },
        ))
        .with_vertical_scroll_bar(create_scroll_bar(
            ctx,
            ScrollBarData {
                min: 0.0,
                max: 0.0,
                value: 0.0,
                step: 0.0,
                row: 0,
                column: 0,
                margin: Default::default(),
                show_value: false,
                orientation: Orientation::Vertical,
            },
        ))
        .build(ctx)
}
