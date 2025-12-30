import React from 'react';
import { createPortal } from 'react-dom';

interface ModalPortalProps {
    children: React.ReactNode;
}

export const ModalPortal: React.FC<ModalPortalProps> = ({ children }) => {
    // Rendern into document body to escape any stacking context
    return createPortal(
        children,
        document.body
    );
};
