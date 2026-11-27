import core;

#include "legacy.h"
#include "util.h"

#include <iostream>

int consume(int value);

int main() {
    const core::Vec<double> vector{1.5, 2.5};
    const core::Point a{0.0, 0.0};
    const core::Point b{3.0, 4.0};
    const Legacy legacy{"legacy", core::add(2, 3)};
    const core::Ints values = core::build(4);

    std::cout << "vec: " << vector.sum() << "\n"
              << "distance: " << core::distance(a, b) << "\n"
              << "legacy: " << legacy.name << " " << legacy_value(legacy) << "\n"
              << "label: " << core::label(legacy.value) << "\n"
              << "detail: " << core::use_detail(5) << "\n"
              << "consumer: " << consume(static_cast<int>(values.size())) << "\n"
              << "util: " << util::tag(7) << " "
              << util::clamp(12, 0, 10) << "\n"
              << "instantiated: " << core::instantiated_sum() << "\n";
    return 0;
}
