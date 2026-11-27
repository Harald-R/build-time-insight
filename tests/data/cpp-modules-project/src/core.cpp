module;
#include <string>
#include <vector>
module core;

import :detail;

namespace core {

int add(int a, int b) { return a + b; }

std::string label(int value) { return "label:" + std::to_string(value); }

int use_detail(int value) { return detail::twice(value); }

Ints build(int count) {
    Ints values;
    for (int i = 0; i < count; ++i) {
        values.push_back(i);
    }
    return values;
}

} // namespace core
