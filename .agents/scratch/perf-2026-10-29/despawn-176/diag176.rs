//! TEMPORARY #176 instrumentation: names every zero-vertex mesh the renderer is handed.

use bevy::prelude::*;
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::ExtractedAssets;
use bevy::render::{Render, RenderApp, RenderSystems};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Last, name_empty_meshes);
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.add_systems(
            Render,
            predict_allocator_errors
                .in_set(RenderSystems::PrepareAssets)
                .before(bevy::render::mesh::allocator::allocate_and_free_meshes),
        );
    }
}

fn vertex_count(mesh: &Mesh) -> Option<usize> {
    let attrs = mesh.try_attributes().ok()?;
    Some(attrs.map(|(_, v)| v.len()).min().unwrap_or(0))
}

fn name_empty_meshes(
    mut events: MessageReader<AssetEvent<Mesh>>,
    meshes: Res<Assets<Mesh>>,
    holders: Query<(Entity, &Mesh3d, Option<&Name>, Option<&ChildOf>)>,
    names: Query<&Name>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else {
            continue;
        };
        let Some(mesh) = meshes.get(*id) else {
            continue;
        };
        let ribbon_layout = mesh
            .try_attributes()
            .map(|mut a| a.any(|(a, _)| a.name == "Vertex_Color"))
            .unwrap_or(false)
            && mesh.asset_usage.contains(bevy::asset::RenderAssetUsages::MAIN_WORLD);
        if matches!(event, AssetEvent::Added { .. })
            && ribbon_layout
            && let Some(n) = vertex_count(mesh)
            && n > 0
        {
            warn!("DIAG176 main: ribbon-layout mesh {id:?} added with {n} vertices");
        }
        if vertex_count(mesh) != Some(0) {
            continue;
        }
        let users: Vec<String> = holders
            .iter()
            .filter(|(_, m, _, _)| m.id() == *id)
            .map(|(e, _, n, p)| {
                format!(
                    "{e} {:?} parent {:?}",
                    n.map(|n| n.as_str().to_owned()),
                    p.and_then(|p| names.get(p.parent()).ok())
                        .map(|n| n.as_str().to_owned())
                )
            })
            .collect();
        let attrs: Vec<String> = mesh
            .try_attributes()
            .map(|a| a.map(|(a, _)| a.name.to_string()).collect())
            .unwrap_or_default();
        let indices = mesh.try_indices_option().ok().flatten().map(|i| i.len());
        warn!(
            "DIAG176 main: zero-vertex mesh {id:?} {} attrs {attrs:?} indices {indices:?} usage {:?} users {users:?}",
            if matches!(event, AssetEvent::Added { .. }) {
                "added"
            } else {
                "modified"
            },
            mesh.asset_usage,
        );
    }
}

fn predict_allocator_errors(extracted: Res<ExtractedAssets<RenderMesh>>) {
    let mut predicted = 0;
    for (id, mesh) in &extracted.extracted {
        if mesh.get_vertex_buffer_size() == 0 {
            let errors = 1 + usize::from(mesh.get_index_buffer_bytes().is_some());
            predicted += errors;
            warn!("DIAG176 render: zero-size vertex buffer {id:?} -> {errors} predicted");
        }
    }
    if predicted > 0 {
        warn!(
            "DIAG176 render: {predicted} predicted allocator errors among {} extracted, {} removed",
            extracted.extracted.len(),
            extracted.removed.len()
        );
    }
}
