#include "fwd_class.h" // probe header: Widget is constructed below
#include "geometry.h"  // -> vector3.h

#include <vector>

// Constructs a Widget and reads a member, requiring its definition. The
// header-usage analysis must classify fwd_class.h as "NeedsDefinition" for
// this unit.
namespace geom {

double edge_needs_work(const std::vector<Vector3<double>> &pts) {
  Widget w;
  const Vector3<double> c = centroid(pts);
  return c.x() + static_cast<double>(w.size());
}

} // namespace geom

double edge_needs_entry() {
  return geom::edge_needs_work({{1, 0, 0}, {2, 0, 0}});
}