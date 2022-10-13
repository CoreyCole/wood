use crate::{effects::EffectKind, fyrox::core::math::Vector3Ext, message::Message, GameTime};
use fyrox::{
    core::{
        algebra::Vector3,
        pool::{Handle, Pool},
        visitor::{Visit, VisitResult, Visitor},
    },
    engine::resource_manager::ResourceManager,
    scene::{
        base::BaseBuilder, graph::Graph, node::Node, pivot::PivotBuilder,
        transform::TransformBuilder, Scene,
    },
};
use std::{path::Path, sync::mpsc::Sender};

#[derive(Copy, Clone, PartialEq, Eq, Debug, Visit)]
pub enum ItemKind {
    Medkit,

    // Ammo
    Plasma,
    Ak47Ammo,
    M4Ammo,

    // Weapons
    PlasmaGun,
    Ak47,
    M4,
    RocketLauncher,
    BattleAxe,
}

#[derive(Visit)]
pub struct Item {
    kind: ItemKind,
    pivot: Handle<Node>,
    model: Handle<Node>,
    offset: Vector3<f32>,
    dest_offset: Vector3<f32>,
    offset_factor: f32,
    reactivation_timer: f32,
    active: bool,
    #[visit(skip)]
    pub sender: Option<Sender<Message>>,
    lifetime: Option<f32>,
}

impl Default for Item {
    fn default() -> Self {
        Self {
            kind: ItemKind::Medkit,
            pivot: Default::default(),
            model: Default::default(),
            offset: Default::default(),
            dest_offset: Default::default(),
            offset_factor: 0.0,
            reactivation_timer: 0.0,
            active: true,
            sender: None,
            lifetime: None,
        }
    }
}

pub struct ItemDefinition {
    model: &'static str,
    scale: f32,
    reactivation_interval: f32,
}

impl Item {
    pub fn get_definition(kind: ItemKind) -> &'static ItemDefinition {
        match kind {
            ItemKind::BattleAxe => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/ak47.FBX",
                    scale: 3.0,
                    reactivation_interval: 30.0,
                };
                &DEFINITION
            }
            ItemKind::Medkit => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/medkit.fbx",
                    scale: 1.0,
                    reactivation_interval: 20.0,
                };
                &DEFINITION
            }
            ItemKind::Plasma => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/yellow_box.FBX",
                    scale: 0.25,
                    reactivation_interval: 15.0,
                };
                &DEFINITION
            }
            ItemKind::Ak47Ammo => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/box_medium.FBX",
                    scale: 0.30,
                    reactivation_interval: 14.0,
                };
                &DEFINITION
            }
            ItemKind::M4Ammo => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/box_small.FBX",
                    scale: 0.30,
                    reactivation_interval: 13.0,
                };
                &DEFINITION
            }
            ItemKind::PlasmaGun => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/plasma_rifle.FBX",
                    scale: 3.0,
                    reactivation_interval: 30.0,
                };
                &DEFINITION
            }
            ItemKind::Ak47 => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/ak47.FBX",
                    scale: 3.0,
                    reactivation_interval: 30.0,
                };
                &DEFINITION
            }
            ItemKind::M4 => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/m4.FBX",
                    scale: 3.0,
                    reactivation_interval: 30.0,
                };
                &DEFINITION
            }
            ItemKind::RocketLauncher => {
                static DEFINITION: ItemDefinition = ItemDefinition {
                    model: "data/models/Rpg7.FBX",
                    scale: 3.0,
                    reactivation_interval: 30.0,
                };
                &DEFINITION
            }
        }
    }

    pub async fn new(
        kind: ItemKind,
        position: Vector3<f32>,
        scene: &mut Scene,
        resource_manager: ResourceManager,
        sender: Sender<Message>,
    ) -> Self {
        let definition = Self::get_definition(kind);

        let model = resource_manager
            .request_model(Path::new(definition.model))
            .await
            .unwrap()
            .instantiate_geometry(scene);

        let pivot = PivotBuilder::new(
            BaseBuilder::new().with_local_transform(
                TransformBuilder::new()
                    .with_local_position(position)
                    .with_local_scale(Vector3::new(
                        definition.scale,
                        definition.scale,
                        definition.scale,
                    ))
                    .build(),
            ),
        )
        .build(&mut scene.graph);

        scene.graph.link_nodes(model, pivot);

        Self {
            pivot,
            kind,
            model,
            sender: Some(sender),
            ..Default::default()
        }
    }

    pub fn get_pivot(&self) -> Handle<Node> {
        self.pivot
    }

    pub fn position(&self, graph: &Graph) -> Vector3<f32> {
        graph[self.pivot].global_position()
    }

    pub fn update(&mut self, graph: &mut Graph, time: GameTime) {
        self.offset_factor += 1.2 * time.delta;

        let amp = 0.085;
        self.dest_offset = Vector3::new(0.0, amp + amp * self.offset_factor.sin(), 0.0);
        self.offset.follow(&self.dest_offset, 0.2);

        let position = graph[self.pivot].global_position();

        let model = &mut graph[self.model];
        model.set_visibility(!self.is_picked_up());
        model.local_transform_mut().set_position(self.offset);

        if !self.active {
            self.reactivation_timer -= time.delta;
            if self.reactivation_timer <= 0.0 {
                self.active = true;

                self.sender
                    .as_ref()
                    .unwrap()
                    .send(Message::CreateEffect {
                        kind: EffectKind::ItemAppear,
                        position,
                    })
                    .unwrap();
            }
        }
    }

    pub fn get_kind(&self) -> ItemKind {
        self.kind
    }

    pub fn definition(&self) -> &'static ItemDefinition {
        Self::get_definition(self.kind)
    }

    pub fn pick_up(&mut self) {
        self.reactivation_timer = self.definition().reactivation_interval;
        self.active = false;
    }

    pub fn is_picked_up(&self) -> bool {
        !self.active
    }

    fn cleanup(&self, graph: &mut Graph) {
        graph.remove_node(self.pivot)
    }

    fn can_be_removed(&self) -> bool {
        match self.lifetime {
            None => false,
            Some(time) => time <= 0.0 || !self.active,
        }
    }

    pub fn set_lifetime(&mut self, lifetime: Option<f32>) {
        self.lifetime = lifetime;
    }
}

#[derive(Visit)]
pub struct ItemContainer {
    pool: Pool<Item>,
}

impl Default for ItemContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl ItemContainer {
    pub fn new() -> Self {
        Self { pool: Pool::new() }
    }

    pub fn add(&mut self, item: Item) -> Handle<Item> {
        self.pool.spawn(item)
    }

    pub fn get_mut(&mut self, item: Handle<Item>) -> &mut Item {
        self.pool.borrow_mut(item)
    }

    pub fn contains(&self, item: Handle<Item>) -> bool {
        self.pool.is_valid_handle(item)
    }

    pub fn pair_iter(&self) -> impl Iterator<Item = (Handle<Item>, &Item)> {
        self.pool.pair_iter()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Item> {
        self.pool.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Item> {
        self.pool.iter_mut()
    }

    pub fn update(&mut self, scene: &mut Scene, time: GameTime) {
        for item in self.pool.iter_mut() {
            item.update(&mut scene.graph, time);
        }

        // Remove temporary items.
        for item in self.pool.iter() {
            if item.can_be_removed() {
                item.cleanup(&mut scene.graph);
            }
        }
        self.pool.retain(|i| !i.can_be_removed())
    }
}
