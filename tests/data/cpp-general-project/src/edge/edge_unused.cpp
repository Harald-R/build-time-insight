#include "geometry.h" // -> vector3.h
#include "shapes.h"   // -> geometry.h -> vector3.h
#include "unused.h" // probe header (first include): included but never referenced below

#include <vector>

// Does enough real work that clang emits the full frontend event tree. The
// probe header unused.h is included but nothing from it is referenced, so the
// header-usage analysis must classify it as "Unused" for this unit.
namespace {

double edge_unused_work() {
  using namespace geom;
  const std::vector<Vector3<double>> pts = {{1, 0, 0}, {0, 1, 0}, {0, 0, 1}};
  const Vector3<double> c = centroid(pts);
  const Sphere s{c, 2.0};
  return volume(s) + c.x();
}

} // namespace

double edge_unused_entry() { return edge_unused_work(); }