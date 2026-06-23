import React from 'react';
import { Button, Tooltip } from 'antd';
import type { ButtonProps } from 'antd';

interface IconButtonProps extends Omit<ButtonProps, 'title'> {
  title?: string;
  icon: React.ReactNode;
}

const IconButton: React.FC<IconButtonProps> = ({
  title,
  icon,
  size = 'small',
  type = 'default',
  ...restProps
}) => {
  const button = (
    <Button icon={icon} size={size} type={type} {...restProps} />
  );

  if (title) {
    return (
      <Tooltip title={title}>
        {button}
      </Tooltip>
    );
  }

  return button;
};

export default IconButton;
