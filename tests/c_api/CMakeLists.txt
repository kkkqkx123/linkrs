cmake_minimum_required(VERSION 3.10)
project(graphdb_c_api_tests C)

# 设置 C 标准
set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

# 设置编译选项
if(MSVC)
    add_compile_options(/W4)
else()
    add_compile_options(-Wall -Wextra -Werror)
endif()

# 设置包含目录
include_directories(
    ${CMAKE_SOURCE_DIR}/../../include
)

# 查找 GraphDB 库
# 注意：这里需要根据实际情况调整库的路径和名称
# 假设 GraphDB 编译为静态库 graphdb.lib (Windows) 或 libgraphdb.a (Linux)

# 设置库路径
set(GRAPHDB_LIB_DIR ${CMAKE_SOURCE_DIR}/../../target/debug)
set(GRAPHDB_INCLUDE_DIR ${CMAKE_SOURCE_DIR}/../../include)

# 链接 GraphDB 库
link_directories(${GRAPHDB_LIB_DIR})

# 添加测试可执行文件
add_executable(graphdb_c_api_tests
    tests.c
)

# 链接库
if(WIN32)
    target_link_libraries(graphdb_c_api_tests
        graphdb
        ws2_32
    )
else()
    target_link_libraries(graphdb_c_api_tests
        graphdb
        pthread
        dl
        m
    )
endif()

# 添加测试
enable_testing()
add_test(NAME graphdb_c_api_tests COMMAND graphdb_c_api_tests)

# 设置输出目录
set_target_properties(graphdb_c_api_tests PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin
)

# 打印配置信息
message(STATUS "GraphDB 库目录: ${GRAPHDB_LIB_DIR}")
message(STATUS "GraphDB 包含目录: ${GRAPHDB_INCLUDE_DIR}")
