//! Self-contained static HTML graph visualization export.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

/// Exports a self-contained static HTML viewer (`graph.html`) with embedded node/edge JSON.
pub fn export_html(graph: &CodeGraph, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    let nodes: Vec<_> = graph
        .nodes
        .values()
        .filter(|n| n.kind == NodeKind::File)
        .take(400)
        .map(|n| serde_json::json!({ "id": n.id, "label": n.label, "community": n.community.unwrap_or(0) }))
        .collect();
    let edges: Vec<_> = graph
        .edges
        .iter()
        .filter(|e| !e.source.is_empty() && e.resolved && e.kind != GraphEdgeKind::Contains)
        .take(800)
        .map(|e| serde_json::json!({ "source": e.source, "target": e.target }))
        .collect();
    let payload = serde_json::json!({ "nodes": nodes, "edges": edges }).to_string();
    let html = format!(
        "<!doctype html><html><head><meta charset='utf-8'><title>Anvil Code Graph</title>\
        <style>body{{margin:0;background:#0f172a;color:#e2e8f0;font-family:sans-serif}}canvas{{width:100vw;height:100vh;display:block}}</style>\
        </head><body><canvas id='c'></canvas><script>const G={payload};const c=document.getElementById('c');\
        const x=c.getContext('2d');c.width=innerWidth;c.height=innerHeight;\
        const pos={{}};G.nodes.forEach((n,i)=>{{const a=i*0.35;const r=80+i*1.2;pos[n.id]=[c.width/2+Math.cos(a)*r%((c.width/2)-20),c.height/2+Math.sin(a)*r%((c.height/2)-20)];}});\
        x.strokeStyle='rgba(148,163,184,0.25)';G.edges.forEach(e=>{{const s=pos[e.source],t=pos[e.target];if(s&&t){{x.beginPath();x.moveTo(s[0],s[1]);x.lineTo(t[0],t[1]);x.stroke();}}}});\
        G.nodes.forEach(n=>{{const p=pos[n.id];x.fillStyle=`hsl(${{(n.community*53)%360}},70%,60%)`;x.beginPath();x.arc(p[0],p[1],4,0,6.28);x.fill();}});\
        </script></body></html>"
    );
    fs::write(path, html).with_context(|| format!("failed to write {}", path.display()))
}
