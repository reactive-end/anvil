//! Wave V3-2 tests: exact `SymbolDef.end_line` spans, HTTP route normalization,
//! server `ExposesRoute` (NestJS, FastAPI/Flask), and client `CallsRoute` (TS/JS, Kotlin).

use std::path::Path;

use anvil_ast::analyze_file;
use anvil_ast::extractors::http_route::{join_route_paths, normalize_route_target};
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, SymbolDef, SymbolKind};

fn cfg() -> AnvilConfig {
    AnvilConfig::default()
}

fn sym<'a>(symbols: &'a [SymbolDef], qname: &str) -> &'a SymbolDef {
    symbols
        .iter()
        .find(|s| s.qualified_name == qname)
        .unwrap_or_else(|| panic!("missing symbol {qname} in {symbols:?}"))
}

fn assert_spans(path: &str, src: &str, expected: &[(&str, u32, u32)]) {
    let facts = analyze_file(Path::new(path), src, &cfg()).expect(path);
    assert!(facts.symbols.iter().all(|s| s.end_line >= s.line));
    for &(qname, line, end_line) in expected {
        let found = sym(&facts.symbols, qname);
        assert_eq!((found.line, found.end_line), (line, end_line), "{path}::{qname}");
    }
}

#[test]
fn symbol_end_line_spans_are_exact_across_ts_python_rust_kotlin_and_dart() {
    assert_spans(
        "src/svc.ts",
        "export function compute(x: number) {\n  return x + 1;\n}\n\nexport class Service {\n  run() {\n    return compute(1);\n  }\n}\n",
        &[("compute", 1, 3), ("Service", 5, 9), ("Service.run", 6, 8)],
    );
    assert_spans(
        "app/repo.py",
        "class OrderRepo:\n    def find_one(self, order_id):\n        return order_id\n\ndef health_check():\n    return True\n",
        &[("OrderRepo", 1, 3), ("OrderRepo.find_one", 2, 3), ("health_check", 5, 6)],
    );
    assert_spans(
        "src/order.rs",
        "pub struct Order {\n    pub id: u64,\n}\n\nimpl Order {\n    pub fn get_id(&self) -> u64 {\n        self.id\n    }\n}\n\npub fn top_helper() {\n    let _ = 1;\n}\n",
        &[("Order", 1, 3), ("impl Order", 5, 9), ("Order::get_id", 6, 8), ("top_helper", 11, 13)],
    );
    assert_spans(
        "app/Repo.kt",
        "class Repo {\n    fun fetch(id: String): String {\n        return id\n    }\n}\n",
        &[("Repo", 1, 5), ("Repo.fetch", 2, 4)],
    );
    assert_spans(
        "lib/card.dart",
        "class OrderCard {\n  int total(int qty) {\n    return qty * 2;\n  }\n}\n\nint topHelper() {\n  return 42;\n}\n",
        &[("OrderCard", 1, 5), ("OrderCard.total", 2, 4), ("topHelper", 7, 9)],
    );
}

#[test]
fn http_route_normalization_unifies_backend_and_frontend_shapes() {
    assert_eq!(join_route_paths("orders", ":id"), "/orders/:id");
    assert_eq!(join_route_paths("/orders/", "/"), "/orders");
    assert_eq!(join_route_paths("", ""), "/");

    let expected = Some("route:GET /orders/:param".to_owned());
    assert_eq!(normalize_route_target("get", "/orders/:id"), expected);
    assert_eq!(normalize_route_target("GET", "/orders/{order_id}"), expected);
    assert_eq!(normalize_route_target("GET", "${this.apiUrl}/orders/${id}"), expected);
    assert_eq!(normalize_route_target("GET", "${API_URL}/orders/${order.id}?expand=1"), expected);
    assert_eq!(normalize_route_target("GET", "https://api.example.com//orders/:id/"), expected);
    assert_eq!(
        normalize_route_target("GET", "http://localhost:3000/orders/<int:id>#top"),
        expected
    );
    assert_eq!(normalize_route_target("GET", "orders/{id}"), expected);

    assert_eq!(normalize_route_target("post", "/"), Some("route:POST /".to_owned()));
    assert_eq!(normalize_route_target("all", "/health"), Some("route:ANY /health".to_owned()));
    assert_eq!(normalize_route_target("GET", "${BASE_URL}"), None);
    assert_eq!(normalize_route_target("GET", ""), None);
}

#[test]
fn nestjs_controller_and_method_decorators_emit_exposes_route() {
    let src = r#"
@Controller('orders')
export class OrdersController {
  @Get()
  listAll() { return []; }

  @Get(':id')
  findOne(@Param('id') id: string) { return id; }

  @Post()
  create() { return {}; }

  @Put(':id')
  replace() { return {}; }

  @Patch(':id/status')
  updateStatus() { return {}; }

  @Delete(':id')
  remove() {}
}
"#;
    let facts = analyze_file(Path::new("src/orders.controller.ts"), src, &cfg()).expect("nest");
    let routes: Vec<(&str, &str)> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::ExposesRoute)
        .map(|e| (e.source_symbol.as_deref().unwrap_or(""), e.target_path_or_symbol.as_str()))
        .collect();

    assert!(routes.contains(&("OrdersController.listAll", "route:GET /orders")));
    assert!(routes.contains(&("OrdersController.findOne", "route:GET /orders/:param")));
    assert!(routes.contains(&("OrdersController.create", "route:POST /orders")));
    assert!(routes.contains(&("OrdersController.replace", "route:PUT /orders/:param")));
    assert!(
        routes.contains(&("OrdersController.updateStatus", "route:PATCH /orders/:param/status"))
    );
    assert!(routes.contains(&("OrdersController.remove", "route:DELETE /orders/:param")));
}

#[test]
fn python_fastapi_and_flask_emit_exposes_route() {
    let src = r#"
@app.get("/orders/{id}")
def get_order(id: str):
    return {}

@router.post("/orders/{order_id}")
async def create_order(order_id: str):
    return {}

@bp.route('/items', methods=['GET', 'POST'])
def handle_items():
    pass

@cache.get("session_key")
def cached_val():
    return None
"#;
    let facts = analyze_file(Path::new("app/routes.py"), src, &cfg()).expect("py routes");
    let routes: Vec<(&str, &str)> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::ExposesRoute)
        .map(|e| (e.source_symbol.as_deref().unwrap_or(""), e.target_path_or_symbol.as_str()))
        .collect();

    assert!(routes.contains(&("get_order", "route:GET /orders/:param")));
    assert!(routes.contains(&("create_order", "route:POST /orders/:param")));
    assert!(routes.contains(&("handle_items", "route:GET /items")));
    assert!(routes.contains(&("handle_items", "route:POST /items")));
    assert_eq!(routes.len(), 4, "unexpected routes: {routes:?}");
}

#[test]
fn ts_angular_axios_fetch_and_kotlin_retrofit_emit_matching_calls_route() {
    let ts_src = r#"
export class OrdersClient {
  private apiUrl = 'https://api.example.com';

  getOrder(id: string) {
    return this.http.get<Order>(`${this.apiUrl}/orders/${id}`);
  }

  createOrder(payload: unknown) {
    return axios.post('/orders', payload);
  }

  loadRaw(id: string) {
    return fetch('/orders/' + id);
  }

  deleteOrder(id: string) {
    return fetch(`/orders/${id}`, { method: 'DELETE' });
  }

  loadLocal() {
    return fetch('./mock-orders.json');
  }
}
"#;
    let ts_facts =
        analyze_file(Path::new("src/orders.client.ts"), ts_src, &cfg()).expect("ts client");
    let ts_routes: Vec<(&str, &str)> = ts_facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::CallsRoute)
        .map(|e| (e.source_symbol.as_deref().unwrap_or(""), e.target_path_or_symbol.as_str()))
        .collect();

    assert!(ts_routes.contains(&("OrdersClient.getOrder", "route:GET /orders/:param")));
    assert!(ts_routes.contains(&("OrdersClient.createOrder", "route:POST /orders")));
    assert!(ts_routes.contains(&("OrdersClient.loadRaw", "route:ANY /orders/:param")));
    assert!(ts_routes.contains(&("OrdersClient.deleteOrder", "route:DELETE /orders/:param")));
    assert_eq!(ts_routes.len(), 4, "relative path leaked or missing route: {ts_routes:?}");

    let kt_src = r#"
interface OrdersApi {
    @GET("orders/{id}")
    suspend fun getOrder(@Path("id") id: String): Order

    @POST("orders/{orderId}")
    suspend fun submitOrder(@Path("orderId") orderId: String): Unit

    @DELETE("orders/{id}")
    suspend fun deleteOrder(@Path("id") id: String): Unit
}
"#;
    let kt_facts =
        analyze_file(Path::new("app/OrdersApi.kt"), kt_src, &cfg()).expect("kt retrofit");
    let kt_routes: Vec<(&str, &str)> = kt_facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::CallsRoute)
        .map(|e| (e.source_symbol.as_deref().unwrap_or(""), e.target_path_or_symbol.as_str()))
        .collect();

    assert!(kt_routes.contains(&("OrdersApi.getOrder", "route:GET /orders/:param")));
    assert!(kt_routes.contains(&("OrdersApi.submitOrder", "route:POST /orders/:param")));
    assert!(kt_routes.contains(&("OrdersApi.deleteOrder", "route:DELETE /orders/:param")));
    assert_eq!(kt_routes.len(), 3);
    assert!(
        kt_facts.symbols.iter().any(|s| s.name == "OrdersApi" && s.kind == SymbolKind::Interface)
    );
}
