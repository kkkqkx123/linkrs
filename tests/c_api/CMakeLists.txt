cmake_minimum_required(VERSION 3.10)
project(linkrs_c_api_tests C)

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

# 查找 Linkrs 库
# 注意：这里需要根据实际情况调整库的路径和名称
# 假设 Linkrs 编译为静态库 linkrs.lib (Windows) 或 liblinkrs.a (Linux)

# 设置库路径
set(LINKRS_LIB_DIR ${CMAKE_SOURCE_DIR}/../../target/debug)
set(LINKRS_INCLUDE_DIR ${CMAKE_SOURCE_DIR}/../../include)

# 链接 Linkrs 库
link_directories(${LINKRS_LIB_DIR})

# 添加测试可执行文件
add_executable(linkrs_c_api_tests
    tests.c
)

# 链接库
if(WIN32)
    target_link_libraries(linkrs_c_api_tests
        linkrs
        ws2_32
    )
else()
    target_link_libraries(linkrs_c_api_tests
        linkrs
        pthread
        dl
        m
    )
endif()

# 添加测试
enable_testing()
add_test(NAME linkrs_c_api_tests COMMAND linkrs_c_api_tests)

# 设置输出目录
set_target_properties(linkrs_c_api_tests PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY ${CMAKE_BINARY_DIR}/bin
)

# 打印配置信息
message(STATUS "Linkrs 库目录: ${LINKRS_LIB_DIR}")
message(STATUS "Linkrs 包含目录: ${LINKRS_INCLUDE_DIR}")
